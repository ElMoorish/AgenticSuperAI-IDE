//! Start and stop a local `llama-server` for a chosen model file.
//!
//! One server at a time. Output is kept in a small ring so the UI can show why a start failed.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

const LOG_LINES: usize = 200;
const MAX_LINE: usize = 400;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOpts {
    pub engine: String,
    pub model: String,
    /// Name the server reports for the model (`--alias`).
    pub alias: String,
    pub mmproj: Option<String>,
    pub ctx: u32,
    pub ngl: u32,
    pub n_cpu_moe: u32,
    pub port: u16,
}

/// Command-line arguments for llama-server (the same flags as config/start-llama-server.ps1).
pub fn build_args(o: &LaunchOpts) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-m".into(), o.model.clone(),
        "--alias".into(), o.alias.clone(),
        "-ngl".into(), o.ngl.to_string(),
        "--ctx-size".into(), o.ctx.to_string(),
        "--flash-attn".into(), "on".into(),
        "-ctk".into(), "q8_0".into(), "-ctv".into(), "q8_0".into(),
        "--jinja".into(),
        "--host".into(), "127.0.0.1".into(),
        "--port".into(), o.port.to_string(),
    ];
    if o.n_cpu_moe > 0 {
        a.push("--n-cpu-moe".into());
        a.push(o.n_cpu_moe.to_string());
    }
    if let Some(m) = o.mmproj.as_ref().filter(|m| !m.is_empty()) {
        a.push("--mmproj".into());
        a.push(m.clone());
    }
    a
}

/// The program must be a file called llama-server, the model a .gguf file or an Ollama blob, and the port non-privileged.
pub fn validate(o: &LaunchOpts) -> Result<(), String> {
    let exe = crate::models::resolve_engine(&PathBuf::from(&o.engine));
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !exe.is_file() || !stem.starts_with("llama-server") {
        return Err("engine must be an existing llama-server program".into());
    }
    if !PathBuf::from(&o.model).is_file() {
        return Err("model file not found".into());
    }
    if o.port < 1024 {
        return Err("port must be 1024 or higher".into());
    }
    if !(512..=1_048_576).contains(&o.ctx) {
        return Err("context size must be between 512 and 1048576".into());
    }
    if o.alias.trim().is_empty() || o.alias.len() > 120 {
        return Err("model name must be 1 to 120 characters".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// "stopped", "starting", "ready" or "exited".
    pub state: String,
    pub model: Option<String>,
    pub port: Option<u16>,
    pub seconds: u64,
    pub exit_code: Option<i32>,
    pub log: Vec<String>,
}

struct Running {
    child: Child,
    model: String,
    port: u16,
    started: Instant,
    exit: Option<Option<i32>>,
    ready: bool,
}

#[derive(Default)]
pub struct ModelServer {
    run: Mutex<Option<Running>>,
    log: Arc<Mutex<VecDeque<String>>>,
}

fn pump<R: AsyncRead + Unpin + Send + 'static>(r: R, log: Arc<Mutex<VecDeque<String>>>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(mut l)) = lines.next_line().await {
            l.truncate(MAX_LINE);
            let mut g = log.lock().unwrap();
            if g.len() >= LOG_LINES {
                g.pop_front();
            }
            g.push_back(l);
        }
    });
}

impl ModelServer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a server. Stops a previous one first. Must run inside the tokio runtime.
    pub async fn start(&self, o: LaunchOpts) -> Result<(), String> {
        validate(&o)?;
        self.stop().await;
        self.log.lock().unwrap().clear();
        let mut cmd = Command::new(crate::models::resolve_engine(&PathBuf::from(&o.engine)));
        cmd.args(build_args(&o)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| format!("could not start llama-server: {e}"))?;
        if let Some(s) = child.stdout.take() {
            pump(s, self.log.clone());
        }
        if let Some(s) = child.stderr.take() {
            pump(s, self.log.clone());
        }
        *self.run.lock().unwrap() = Some(Running { child, model: o.alias.clone(), port: o.port, started: Instant::now(), exit: None, ready: false });
        Ok(())
    }

    /// Stop the server if one is running (the process is killed, not asked nicely: it holds only a loaded model).
    pub async fn stop(&self) {
        let taken = self.run.lock().unwrap().take();
        if let Some(mut r) = taken {
            let _ = r.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), r.child.wait()).await;
        }
    }

    /// Current state. Checks `/health` while starting (llama-server answers 200 once the model is loaded).
    pub fn status(&self) -> ServerStatus {
        let mut g = self.run.lock().unwrap();
        let log: Vec<String> = self.log.lock().unwrap().iter().rev().take(60).rev().cloned().collect();
        let Some(r) = g.as_mut() else {
            return ServerStatus { state: "stopped".into(), model: None, port: None, seconds: 0, exit_code: None, log };
        };
        if r.exit.is_none() {
            if let Ok(Some(st)) = r.child.try_wait() {
                r.exit = Some(st.code());
            }
        }
        let state = match r.exit {
            Some(_) => "exited",
            None if r.ready => "ready",
            None => {
                if health_ok(r.port) {
                    r.ready = true;
                    "ready"
                } else {
                    "starting"
                }
            }
        };
        ServerStatus {
            state: state.into(),
            model: Some(r.model.clone()),
            port: Some(r.port),
            seconds: r.started.elapsed().as_secs(),
            exit_code: r.exit.flatten(),
            log,
        }
    }
}

fn health_ok(port: u16) -> bool {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(300))
        .build()
        .get(&format!("http://127.0.0.1:{port}/health"))
        .call()
        .map_or(false, |r| r.status() == 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> LaunchOpts {
        LaunchOpts { engine: "x".into(), model: "m.gguf".into(), alias: "m".into(), mmproj: None, ctx: 8192, ngl: 99, n_cpu_moe: 0, port: 8080 }
    }

    #[test]
    fn args_have_the_standard_flags_and_optional_ones() {
        let a = build_args(&opts());
        assert_eq!(&a[0..2], ["-m", "m.gguf"]);
        assert!(a.windows(2).any(|w| w == ["--port", "8080"]));
        assert!(a.windows(2).any(|w| w == ["--host", "127.0.0.1"]));
        assert!(!a.contains(&"--n-cpu-moe".to_string()));
        assert!(!a.contains(&"--mmproj".to_string()));
        let mut o = opts();
        o.n_cpu_moe = 24;
        o.mmproj = Some("p.gguf".into());
        let a = build_args(&o);
        assert!(a.windows(2).any(|w| w == ["--n-cpu-moe", "24"]));
        assert!(a.windows(2).any(|w| w == ["--mmproj", "p.gguf"]));
    }

    #[test]
    fn validation_rejects_bad_input() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join(if cfg!(windows) { "llama-server.exe" } else { "llama-server" });
        let other = d.path().join("notepad.exe");
        let model = d.path().join("m.gguf");
        for p in [&exe, &other, &model] {
            std::fs::write(p, b"x").unwrap();
        }
        let mut o = opts();
        o.engine = exe.to_string_lossy().to_string();
        o.model = model.to_string_lossy().to_string();
        assert!(validate(&o).is_ok());
        let mut b = o.clone();
        b.engine = other.to_string_lossy().to_string();
        assert!(validate(&b).unwrap_err().contains("llama-server"));
        let mut b = o.clone();
        b.model = "nope.gguf".into();
        assert!(validate(&b).unwrap_err().contains("model"));
        let mut b = o.clone();
        b.port = 80;
        assert!(validate(&b).is_err());
        let mut b = o.clone();
        b.ctx = 10;
        assert!(validate(&b).is_err());
        let mut b = o;
        b.alias = " ".into();
        assert!(validate(&b).is_err());
    }

    #[tokio::test]
    async fn stopped_by_default_and_stop_is_harmless() {
        let s = ModelServer::new();
        assert_eq!(s.status().state, "stopped");
        s.stop().await;
        assert_eq!(s.status().state, "stopped");
    }
}
