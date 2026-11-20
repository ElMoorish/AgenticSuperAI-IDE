//! Model endpoint probing and GPU memory query.
//!
//! `probe_endpoint`, `detect_servers` and `gpu_memory` are BLOCKING; async
//! callers must run them via `tokio::task::spawn_blocking`.

use std::io::Read;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub ok: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedServer {
    pub name: String,
    pub url: String,
}

const MAX_BODY: u64 = 4 * 1024 * 1024;

fn fail(msg: impl Into<String>) -> ProbeResult {
    ProbeResult { ok: false, models: Vec::new(), error: Some(msg.into()) }
}

/// Remove `user:pass@` from a URL string.
fn strip_userinfo(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let (scheme, rest) = url.split_at(i + 3);
            let end = rest.find('/').unwrap_or(rest.len());
            match rest[..end].rfind('@') {
                Some(at) => format!("{scheme}{}", &rest[at + 1..]),
                None => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

fn models_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    let b = b.strip_suffix("/v1").unwrap_or(b);
    format!("{b}/v1/models")
}

fn parse_models(v: &serde_json::Value) -> Vec<String> {
    let pick = |arr: &serde_json::Value, keys: &[&str]| -> Vec<String> {
        arr.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| keys.iter().find_map(|k| m.get(k)?.as_str()))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = pick(&v["data"], &["id"]);
    if out.is_empty() {
        out = pick(&v["models"], &["name", "model", "id"]);
    }
    out
}

/// `GET {base}/v1/models` with a 3 s total timeout. Never panics.
pub fn probe_endpoint(base: &str) -> ProbeResult {
    probe_endpoint_with_key(base, "")
}

/// Same, sending `Authorization: Bearer <key>` when the key is not empty.
pub fn probe_endpoint_with_key(base: &str, key: &str) -> ProbeResult {
    probe_with_timeout(base, key, Duration::from_secs(3))
}

fn probe_with_timeout(base: &str, key: &str, timeout: Duration) -> ProbeResult {
    let url = models_url(base);
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .redirects(0)
        .build();
    let mut req = agent.get(&url);
    if !key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", key.trim()));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return fail(format!("server answered HTTP {code}")),
        Err(ureq::Error::Transport(t)) => {
            let shown = strip_userinfo(base.trim().trim_end_matches('/'));
            let msg = match t.kind() {
                ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => {
                    "invalid endpoint URL".to_string()
                }
                ureq::ErrorKind::Dns => format!("cannot resolve host for {shown}"),
                ureq::ErrorKind::ConnectionFailed => format!("cannot connect to {shown}"),
                ureq::ErrorKind::Io => format!("timed out or connection lost ({shown})"),
                _ => format!("request to {shown} failed"),
            };
            return fail(msg);
        }
    };
    if !(200..300).contains(&resp.status()) {
        return fail(format!("server answered HTTP {}", resp.status()));
    }
    let mut body = Vec::new();
    if resp.into_reader().take(MAX_BODY + 1).read_to_end(&mut body).is_err() {
        return fail("could not read response (timed out or connection lost)");
    }
    if body.len() as u64 > MAX_BODY {
        return fail("response too large for a models endpoint");
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return fail("not an OpenAI-style models endpoint");
    };
    if !v["data"].is_array() && !v["models"].is_array() {
        return fail("not an OpenAI-style models endpoint");
    }
    ProbeResult { ok: true, models: parse_models(&v), error: None }
}

/// Probe the usual local servers in parallel (500 ms each); returns the ones
/// that answered `/v1/models` with a 2xx, in fixed order.
pub fn detect_servers() -> Vec<DetectedServer> {
    detect_servers_at(&[
        ("Ollama", "http://127.0.0.1:11434"),
        ("LM Studio", "http://127.0.0.1:1234"),
        ("llama-server", "http://127.0.0.1:8080"),
    ])
}

#[doc(hidden)]
pub fn detect_servers_at(candidates: &[(&str, &str)]) -> Vec<DetectedServer> {
    let handles: Vec<_> = candidates
        .iter()
        .map(|(name, url)| {
            let (name, url) = (name.to_string(), url.to_string());
            std::thread::spawn(move || {
                probe_with_timeout(&url, "", Duration::from_millis(500)).ok.then(|| DetectedServer { name, url })
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
}

/// One reading of the machine's memory and storage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SysSample {
    pub ram_used_mib: u32,
    pub ram_total_mib: u32,
    /// Bytes read from all disks per second since the previous sample, in MB/s (decimal).
    pub disk_read_mbps: f32,
}

/// Convert a byte count over an interval to MB/s; zero for an empty or invalid interval.
pub fn mbps(bytes: u64, secs: f64) -> f32 {
    if secs > 0.0 && secs.is_finite() {
        (bytes as f64 / secs / 1_000_000.0) as f32
    } else {
        0.0
    }
}

/// Keeps the previous sample so disk reads can be turned into a rate.
pub struct SysMonitor {
    sys: sysinfo::System,
    disks: sysinfo::Disks,
    last: std::time::Instant,
}

impl SysMonitor {
    pub fn new() -> Self {
        SysMonitor { sys: sysinfo::System::new(), disks: sysinfo::Disks::new_with_refreshed_list(), last: std::time::Instant::now() }
    }

    /// Blocking but quick (milliseconds).
    pub fn sample(&mut self) -> SysSample {
        self.sys.refresh_memory();
        self.disks.refresh(true);
        let now = std::time::Instant::now();
        let secs = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        let read: u64 = self.disks.list().iter().map(|d| d.usage().read_bytes).sum();
        SysSample {
            ram_used_mib: (self.sys.used_memory() / 1_048_576) as u32,
            ram_total_mib: (self.sys.total_memory() / 1_048_576) as u32,
            disk_read_mbps: mbps(read, secs),
        }
    }
}

impl Default for SysMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the first line of `nvidia-smi ... --format=csv,noheader,nounits`.
pub fn parse_gpu_csv(s: &str) -> Option<(u32, u32)> {
    let line = s.lines().next()?;
    let mut it = line.split(',');
    let used = it.next()?.trim().parse().ok()?;
    let total = it.next()?.trim().parse().ok()?;
    Some((used, total))
}

fn nvidia_smi_path() -> std::ffi::OsString {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let p = std::path::Path::new(&root).join("System32").join("nvidia-smi.exe");
        if p.is_file() {
            return p.into_os_string();
        }
    }
    let p = std::path::Path::new(r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe");
    if p.is_file() {
        return p.as_os_str().to_owned();
    }
    "nvidia-smi".into()
}

/// (used MiB, total MiB) of the first GPU, or `None` on any failure.
/// Blocking, up to 2 s (the process is killed on timeout).
pub fn gpu_memory() -> Option<(u32, u32)> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(nvidia_smi_path());
    cmd.args(["--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = (&mut out).take(64 * 1024).read_to_string(&mut s);
        s
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    parse_gpu_csv(&reader.join().ok()?)
}

#[cfg(test)]
mod sys_tests {
    use super::*;

    #[test]
    fn mbps_converts_and_guards() {
        assert_eq!(mbps(2_000_000, 2.0), 1.0);
        assert_eq!(mbps(5, 0.0), 0.0);
        assert_eq!(mbps(5, -1.0), 0.0);
        assert_eq!(mbps(5, f64::NAN), 0.0);
    }

    #[test]
    fn sample_reports_this_machines_memory() {
        let mut m = SysMonitor::new();
        let a = m.sample();
        assert!(a.ram_total_mib > 0);
        assert!(a.ram_used_mib <= a.ram_total_mib);
        assert!(a.disk_read_mbps >= 0.0);
    }
}
