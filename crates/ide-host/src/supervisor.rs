//! Spawns and supervises the agent process: stdio reader/writer tasks, request/response
//! correlation, crash detection with backoff restarts.

use crate::acp::{self, AcpError, AgentEvent, Incoming};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub exe: PathBuf,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Delay before restart n (clamped to the last entry).
    pub backoff: Vec<Duration>,
    /// Give up after this many crashes within `crash_window`.
    pub max_crashes_in_window: u32,
    pub crash_window: Duration,
}

impl AgentConfig {
    pub fn new(exe: PathBuf, cwd: PathBuf) -> Self {
        AgentConfig {
            exe,
            env: Vec::new(),
            cwd,
            backoff: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ],
            max_crashes_in_window: 5,
            crash_window: Duration::from_secs(60),
        }
    }
}

/// Receiving end of the agent event stream. Unbounded so the reader/lifecycle tasks never
/// block on a slow consumer (which would stall response routing).
pub type EventRx = mpsc::UnboundedReceiver<AgentEvent>;
type EventTx = mpsc::UnboundedSender<AgentEvent>;

type Pending = oneshot::Sender<Result<Value, AcpError>>;

struct State {
    closed: bool,
    /// Line sink of the current process; `None` while no process is running.
    tx: Option<mpsc::UnboundedSender<String>>,
    pending: HashMap<u64, Pending>,
    next_id: u64,
}

impl State {
    fn fail_all(&mut self, err: AcpError) {
        self.tx = None;
        for (_, p) in self.pending.drain() {
            let _ = p.send(Err(err.clone()));
        }
    }
}

struct Inner {
    state: Mutex<State>,
    stop: Notify,
}

pub struct Supervisor {
    inner: Arc<Inner>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    /// Spawn the agent and start supervising it. Must be called inside a tokio runtime.
    ///
    /// Events are queued without bound: the response path never depends on the consumer
    /// draining. The host must coalesce/cap on its side.
    pub fn start(cfg: AgentConfig) -> (Supervisor, EventRx) {
        let inner = Arc::new(Inner {
            state: Mutex::new(State { closed: false, tx: None, pending: HashMap::new(), next_id: 1 }),
            stop: Notify::new(),
        });
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let first = launch(&cfg, &inner, &events_tx);
        let task = tokio::spawn(run(cfg, inner.clone(), events_tx, first));
        (Supervisor { inner, task: tokio::sync::Mutex::new(Some(task)) }, events_rx)
    }

    /// Send a request; resolves when the matching response arrives. Dropping the future
    /// removes the pending entry. There is no timeout of its own: callers of
    /// `initialize` / `session/new` should wrap it in `tokio::time::timeout`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let (otx, orx) = oneshot::channel();
        let guard;
        {
            let mut st = self.inner.state.lock().unwrap();
            if st.closed {
                return Err(AcpError::Closed);
            }
            let Some(tx) = st.tx.clone() else {
                return Err(AcpError::Exited);
            };
            let id = st.next_id;
            st.next_id += 1;
            st.pending.insert(id, otx);
            guard = Some(PendingGuard { inner: self.inner.clone(), id });
            if tx.send(acp::encode_request(id, method, params)).is_err() {
                return Err(AcpError::Exited);
            }
        }
        let r = orx.await.unwrap_or(Err(AcpError::Exited));
        drop(guard);
        r
    }

    /// Fire-and-forget notification (e.g. `session/cancel`). Dropped if no process is running.
    pub fn notify(&self, method: &str, params: Value) {
        self.send_line(acp::encode_notification(method, params));
    }

    /// Answer a request the agent sent us (`AgentEvent::Request`).
    pub fn respond(&self, id: Value, result: Result<Value, Value>) {
        self.send_line(acp::encode_response(id, result));
    }

    #[doc(hidden)]
    pub fn pending_len(&self) -> usize {
        self.inner.state.lock().unwrap().pending.len()
    }

    fn send_line(&self, line: String) {
        let st = self.inner.state.lock().unwrap();
        if let Some(tx) = &st.tx {
            let _ = tx.send(line);
        }
    }

    /// Kill the child and end the event stream (no `GaveUp`).
    pub async fn shutdown(&self) {
        {
            let mut st = self.inner.state.lock().unwrap();
            st.closed = true;
            st.fail_all(AcpError::Closed);
        }
        self.inner.stop.notify_one();
        let task = self.task.lock().await.take();
        if let Some(t) = task {
            let _ = t.await;
        }
    }
}

struct PendingGuard {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.inner.state.lock().unwrap().pending.remove(&self.id);
    }
}

/// Send a lifecycle event unless shutdown has begun (checked under the state lock).
fn emit(inner: &Inner, events: &EventTx, ev: AgentEvent) -> bool {
    let st = inner.state.lock().unwrap();
    if st.closed {
        return false;
    }
    let _ = events.send(ev);
    true
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        {
            let mut st = self.inner.state.lock().unwrap();
            st.closed = true;
            st.fail_all(AcpError::Closed);
        }
        self.inner.stop.notify_one();
    }
}

fn spawn_child(cfg: &AgentConfig) -> std::io::Result<Child> {
    let mut cmd = Command::new(&cfg.exe);
    cmd.envs(cfg.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&cfg.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd.spawn()
}

fn is_closed(inner: &Inner) -> bool {
    inner.state.lock().unwrap().closed
}

struct Running {
    child: Child,
    writer: JoinHandle<()>,
    drain: JoinHandle<()>,
    reader: JoinHandle<()>,
}

impl Running {
    fn abort_tasks(&self) {
        self.writer.abort();
        self.drain.abort();
        self.reader.abort();
    }
}

/// Spawn the child and its I/O tasks, and publish its line sink so requests can be sent
/// immediately. Synchronous so `Supervisor::start` can have a live process on return.
fn launch(cfg: &AgentConfig, inner: &Arc<Inner>, events: &EventTx) -> Option<Running> {
    let mut child = match spawn_child(cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agent spawn failed: {e}");
            return None;
        }
    };
    let (line_tx, line_rx) = mpsc::unbounded_channel::<String>();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let writer = tokio::spawn(write_loop(stdin, line_rx));
    let drain = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await;
    });
    let reader = tokio::spawn(read_loop(stdout, inner.clone(), events.clone()));
    inner.state.lock().unwrap().tx = Some(line_tx);
    Some(Running { child, writer, drain, reader })
}

async fn run(cfg: AgentConfig, inner: Arc<Inner>, events: EventTx, first: Option<Running>) {
    let mut crashes: Vec<Instant> = Vec::new();
    let mut restarts: u32 = 0;
    let mut first = Some(first);
    loop {
        if is_closed(&inner) {
            if let Some(Some(mut r)) = first.take() {
                let _ = r.child.kill().await;
                r.abort_tasks();
            }
            return;
        }
        let running = match first.take() {
            Some(r) => r,
            None => launch(&cfg, &inner, &events),
        };
        let code = match running {
            None => None,
            Some(mut r) => {
                if restarts > 0 {
                    if !emit(&inner, &events, AgentEvent::Restarted { attempt: restarts }) {
                        let _ = r.child.kill().await;
                        r.abort_tasks();
                        return;
                    }
                }
                let status = tokio::select! {
                    s = r.child.wait() => s,
                    _ = inner.stop.notified() => {
                        let _ = r.child.kill().await;
                        r.abort_tasks();
                        inner.state.lock().unwrap().fail_all(AcpError::Closed);
                        return;
                    }
                };
                // Let already-buffered output (final responses) flow before failing pendings.
                if tokio::time::timeout(Duration::from_millis(500), &mut r.reader).await.is_err() {
                    r.reader.abort();
                }
                r.abort_tasks();
                status.ok().and_then(|s| s.code())
            }
        };
        if is_closed(&inner) {
            return;
        }
        inner.state.lock().unwrap().fail_all(AcpError::Exited);
        if !emit(&inner, &events, AgentEvent::Exited { code }) {
            return;
        }

        let now = Instant::now();
        crashes.push(now);
        crashes.retain(|t| now.duration_since(*t) <= cfg.crash_window);
        if crashes.len() as u32 >= cfg.max_crashes_in_window {
            emit(&inner, &events, AgentEvent::GaveUp);
            return;
        }
        let delay = cfg
            .backoff
            .get(crashes.len().saturating_sub(1).min(cfg.backoff.len().saturating_sub(1)))
            .copied()
            .unwrap_or_default();
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = inner.stop.notified() => return,
        }
        restarts += 1;
    }
}

async fn write_loop(mut stdin: tokio::process::ChildStdin, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = rx.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn read_loop(stdout: tokio::process::ChildStdout, inner: Arc<Inner>, events: EventTx) {
    // read_until has no line-length cap, so very long lines are fine.
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = match std::str::from_utf8(&buf) {
            Ok(t) => t.trim(),
            Err(e) => {
                eprintln!("agent line is not valid UTF-8, skipped: {e}");
                continue;
            }
        };
        if text.is_empty() {
            continue;
        }
        match acp::parse_line(text) {
            Err(e) => {
                let head: String = text.chars().take(200).collect();
                eprintln!("agent line skipped ({e}): {head}");
            }
            Ok(Incoming::Response { id, result }) => {
                let p = inner.state.lock().unwrap().pending.remove(&id);
                if let Some(p) = p {
                    let _ = p.send(result.map_err(AcpError::Remote));
                }
            }
            Ok(Incoming::Notification { method, params }) => {
                if method == "session/update" {
                    let session_id = params
                        .get("sessionId")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let update = params.get("update").cloned().unwrap_or(Value::Null);
                    let _ = events.send(AgentEvent::Update { session_id, update });
                }
            }
            Ok(Incoming::Request { id, method, params }) => {
                let _ = events.send(AgentEvent::Request { id, method, params });
            }
        }
    }
}
