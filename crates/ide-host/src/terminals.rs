//! Terminals: agent-owned pipe processes (`AgentTerminals`) and the user's
//! interactive pseudo-terminal (`UserPty`).
//!
//! Agent terminals never go through a shell string: the program is resolved
//! to a full path (honouring PATHEXT so `npm.cmd` works) and arguments are
//! passed as an argv vector. stdout and stderr are merged, in arrival order,
//! into one ring buffer, and output events are coalesced and capped.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

/// Ring buffer capacity for each agent terminal.
pub const AGENT_TERMINAL_BUFFER: usize = 256 * 1024;
/// Largest single `Output` event chunk.
const MAX_CHUNK: usize = 32 * 1024;
/// Flush interval for coalescing.
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// After this many emitted bytes per terminal, `Output` events stop.
const EMISSION_CAP: usize = 1024 * 1024;

/// Terminal handle. A plain `u64` alias, unique within an `AgentTerminals`.
pub type TerminalId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Output { id: TerminalId, chunk: String },
    /// Emitted once when the per-terminal emission cap is hit.
    Truncated { id: TerminalId },
    Exited { id: TerminalId, code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalError {
    NotFound(String),
    Unknown,
    Io(String),
    Pty(String),
}

impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalError::NotFound(p) => write!(f, "program not found: {p}"),
            TerminalError::Unknown => write!(f, "unknown terminal"),
            TerminalError::Io(e) => write!(f, "io error: {e}"),
            TerminalError::Pty(e) => write!(f, "pty error: {e}"),
        }
    }
}

impl std::error::Error for TerminalError {}

// ---------------------------------------------------------------- RingBuffer

/// Keeps the last `cap` bytes pushed to it.
pub struct RingBuffer {
    cap: usize,
    buf: VecDeque<u8>,
    truncated: bool,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self { cap, buf: VecDeque::new(), truncated: false }
    }

    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.cap {
            if !self.buf.is_empty() || data.len() > self.cap {
                self.truncated = true;
            }
            self.buf.clear();
            self.buf.extend(&data[data.len() - self.cap..]);
            return;
        }
        let total = self.buf.len() + data.len();
        if total > self.cap {
            self.buf.drain(..total - self.cap);
            self.truncated = true;
        }
        self.buf.extend(data);
    }

    /// Returns the buffered tail as valid UTF-8 and whether anything was dropped.
    /// If the cut landed inside a multi-byte sequence the partial leading bytes
    /// are dropped; invalid bytes elsewhere become U+FFFD.
    pub fn snapshot(&self) -> (String, bool) {
        let bytes: Vec<u8> = self.buf.iter().copied().collect();
        let mut start = 0;
        if self.truncated {
            while start < bytes.len() && start < 3 && (bytes[start] & 0xC0) == 0x80 {
                start += 1;
            }
        }
        (String::from_utf8_lossy(&bytes[start..]).into_owned(), self.truncated)
    }
}

// ----------------------------------------------------------- resolve_program

/// Resolves `cmd` to a launchable path. Names with a path separator are used
/// as given; bare names are searched on PATH (or `path_override`) trying PATHEXT.
pub(crate) fn resolve_program(
    cmd: &str,
    path_override: Option<OsString>,
) -> Result<PathBuf, TerminalError> {
    let not_found = || TerminalError::NotFound(cmd.to_string());
    if cmd.is_empty() {
        return Err(not_found());
    }
    if cmd.contains('/') || cmd.contains('\\') {
        return Ok(PathBuf::from(cmd));
    }
    let path = path_override.or_else(|| std::env::var_os("PATH")).ok_or_else(not_found)?;
    let has_ext = Path::new(cmd).extension().is_some();
    let exts: Vec<String> = if cfg!(windows) && !has_ext {
        let pe = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pe.split(';').filter(|e| !e.is_empty()).map(|e| e.to_string()).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &exts {
            let cand = dir.join(format!("{cmd}{ext}"));
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    Err(not_found())
}

// ------------------------------------------------------------ AgentTerminals

struct Entry {
    ring: Arc<Mutex<RingBuffer>>,
    exit: watch::Receiver<Option<Option<i32>>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

/// Agent-owned processes with piped output. Must be used inside a tokio runtime.
pub struct AgentTerminals {
    events: UnboundedSender<TerminalEvent>,
    next: AtomicU64,
    terms: Mutex<HashMap<TerminalId, Arc<Entry>>>,
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}
#[cfg(not(windows))]
fn kill_tree(_pid: u32) {}

impl AgentTerminals {
    pub fn new(events: UnboundedSender<TerminalEvent>) -> Self {
        Self { events, next: AtomicU64::new(1), terms: Mutex::new(HashMap::new()) }
    }

    pub fn create(
        &self,
        cmd: &str,
        args: &[String],
        cwd: &Path,
    ) -> Result<TerminalId, TerminalError> {
        let program = resolve_program(cmd, None)?;
        let mut c = Command::new(&program);
        c.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        c.creation_flags(0x0800_0000);
        let mut child = c.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TerminalError::NotFound(cmd.to_string())
            } else {
                TerminalError::Io(e.to_string())
            }
        })?;

        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let ring = Arc::new(Mutex::new(RingBuffer::new(AGENT_TERMINAL_BUFFER)));
        let emitted = Arc::new(AtomicUsize::new(0));
        let capped = Arc::new(AtomicBool::new(false));
        let mut readers = Vec::new();
        if let Some(o) = child.stdout.take() {
            readers.push(tokio::spawn(read_stream(
                o,
                id,
                ring.clone(),
                self.events.clone(),
                emitted.clone(),
                capped.clone(),
            )));
        }
        if let Some(e) = child.stderr.take() {
            readers.push(tokio::spawn(read_stream(
                e,
                id,
                ring.clone(),
                self.events.clone(),
                emitted,
                capped,
            )));
        }

        let (exit_tx, exit_rx) = watch::channel(None);
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let events = self.events.clone();
        let pid = child.id();
        tokio::spawn(async move {
            let code = tokio::select! {
                s = child.wait() => s.ok().and_then(|s| s.code()),
                _ = kill_rx => {
                    if let Some(pid) = pid {
                        let _ = tokio::task::spawn_blocking(move || kill_tree(pid)).await;
                    }
                    let _ = child.kill().await;
                    child.wait().await.ok().and_then(|s| s.code())
                }
            };
            // Let readers drain; grandchildren holding the pipes open must not hang us.
            for r in readers {
                let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
            }
            let _ = events.send(TerminalEvent::Exited { id, code });
            let _ = exit_tx.send(Some(code));
        });

        let entry = Arc::new(Entry { ring, exit: exit_rx, kill: Mutex::new(Some(kill_tx)) });
        self.terms.lock().unwrap().insert(id, entry);
        Ok(id)
    }

    fn get(&self, id: TerminalId) -> Result<Arc<Entry>, TerminalError> {
        self.terms.lock().unwrap().get(&id).cloned().ok_or(TerminalError::Unknown)
    }

    pub async fn wait_for_exit(&self, id: TerminalId) -> Result<ExitStatus, TerminalError> {
        let entry = self.get(id)?;
        let mut rx = entry.exit.clone();
        let v = rx.wait_for(|v| v.is_some()).await.map_err(|_| TerminalError::Unknown)?;
        Ok(ExitStatus { code: v.flatten() })
    }

    pub fn output(&self, id: TerminalId) -> Result<(String, bool), TerminalError> {
        Ok(self.get(id)?.ring.lock().unwrap().snapshot())
    }

    /// Kills a still-running process (the whole tree via `taskkill /T` on
    /// Windows; best effort, a grandchild that escapes may survive) and
    /// forgets the terminal.
    pub fn release(&self, id: TerminalId) {
        let entry = self.terms.lock().unwrap().remove(&id);
        if let Some(entry) = entry {
            if let Some(tx) = entry.kill.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    }
}

/// Length of the longest prefix of `b` that does not end in an incomplete
/// UTF-8 sequence (invalid bytes count as complete; they are decoded lossily).
fn complete_prefix(b: &[u8]) -> usize {
    let mut b = b;
    let mut off = 0;
    loop {
        match std::str::from_utf8(b) {
            Ok(_) => return off + b.len(),
            Err(e) => match e.error_len() {
                None => return off + e.valid_up_to(),
                Some(n) => {
                    let skip = e.valid_up_to() + n;
                    off += skip;
                    b = &b[skip..];
                }
            },
        }
    }
}

fn flush(
    pending: &mut Vec<u8>,
    all: bool,
    id: TerminalId,
    events: &UnboundedSender<TerminalEvent>,
    emitted: &AtomicUsize,
    capped: &AtomicBool,
) {
    while !pending.is_empty() {
        let n = pending.len().min(MAX_CHUNK);
        let cut = if all && n == pending.len() { n } else { complete_prefix(&pending[..n]) };
        if cut == 0 {
            break;
        }
        let chunk: Vec<u8> = pending.drain(..cut).collect();
        if emitted.load(Ordering::Relaxed) >= EMISSION_CAP {
            if !capped.swap(true, Ordering::SeqCst) {
                let _ = events.send(TerminalEvent::Truncated { id });
            }
            continue;
        }
        emitted.fetch_add(chunk.len(), Ordering::Relaxed);
        let _ = events.send(TerminalEvent::Output {
            id,
            chunk: String::from_utf8_lossy(&chunk).into_owned(),
        });
    }
}

async fn read_stream<R: AsyncRead + Unpin>(
    mut r: R,
    id: TerminalId,
    ring: Arc<Mutex<RingBuffer>>,
    events: UnboundedSender<TerminalEvent>,
    emitted: Arc<AtomicUsize>,
    capped: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let n = match deadline {
            None => r.read(&mut buf).await.ok(),
            Some(d) => match tokio::time::timeout_at(d, r.read(&mut buf)).await {
                Ok(res) => res.ok(),
                Err(_) => {
                    flush(&mut pending, false, id, &events, &emitted, &capped);
                    deadline = if pending.is_empty() {
                        None
                    } else {
                        Some(Instant::now() + FLUSH_INTERVAL)
                    };
                    continue;
                }
            },
        };
        match n {
            Some(n) if n > 0 => {
                ring.lock().unwrap().push(&buf[..n]);
                pending.extend_from_slice(&buf[..n]);
                if pending.len() >= MAX_CHUNK {
                    flush(&mut pending, false, id, &events, &emitted, &capped);
                }
                if deadline.is_none() && !pending.is_empty() {
                    deadline = Some(Instant::now() + FLUSH_INTERVAL);
                }
            }
            _ => break,
        }
    }
    flush(&mut pending, true, id, &events, &emitted, &capped);
}

// ------------------------------------------------------------------- UserPty

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(Option<i32>),
}

/// The user's interactive shell on a pseudo-terminal (ConPTY on Windows).
pub struct UserPty {
    master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

fn pty_err<E: std::fmt::Display>(e: E) -> TerminalError {
    TerminalError::Pty(e.to_string())
}

impl UserPty {
    pub fn spawn(
        shell: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        events: UnboundedSender<PtyEvent>,
    ) -> Result<UserPty, TerminalError> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(pty_err)?;
        let program = resolve_program(shell, None)?;
        let mut cb = CommandBuilder::new(program);
        cb.cwd(cwd);
        let mut child = pair.slave.spawn_command(cb).map_err(pty_err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
        let writer = pair.master.take_writer().map_err(pty_err)?;
        let killer = child.clone_killer();

        // Reader: blocking, forwards chunks of up to 8 KB, ends at EOF/error.
        let tx = events.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    // Keep draining even if the receiver is gone: ConPTY teardown
                    // can block while its output is not being read.
                    Ok(n) => {
                        let _ = tx.send(PtyEvent::Output(buf[..n].to_vec()));
                    }
                }
            }
        });
        // Waiter: ConPTY readers may not reach EOF when the shell exits, so
        // exit is detected by polling the child. A short grace lets the final
        // output reach the channel first.
        std::thread::spawn(move || loop {
            match child.try_wait() {
                Ok(Some(st)) => {
                    std::thread::sleep(Duration::from_millis(150));
                    let _ = events.send(PtyEvent::Exited(Some(st.exit_code() as i32)));
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = events.send(PtyEvent::Exited(None));
                    break;
                }
            }
        });

        Ok(UserPty {
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<(), TerminalError> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| TerminalError::Io(e.to_string()))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), TerminalError> {
        match self.master.lock().unwrap().as_ref() {
            Some(m) => m
                .resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .map_err(pty_err),
            None => Err(TerminalError::Unknown),
        }
    }

    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap().kill();
    }
}

impl Drop for UserPty {
    fn drop(&mut self) {
        self.kill();
        // Closing the pseudo-console can block; do it off the caller's thread.
        if let Some(m) = self.master.lock().unwrap().take() {
            std::thread::spawn(move || drop(m));
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn resolves_cmd_shim_on_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.cmd"), "@echo hi\r\n").unwrap();
        let p = resolve_program("hello", Some(dir.path().as_os_str().to_owned())).unwrap();
        assert!(p.to_string_lossy().to_lowercase().ends_with("hello.cmd"), "{p:?}");
        assert!(p.starts_with(dir.path()));
    }

    #[test]
    fn unknown_program_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve_program("nope-nope", Some(dir.path().as_os_str().to_owned()));
        assert!(matches!(r, Err(TerminalError::NotFound(_))));
    }

    #[tokio::test]
    async fn shim_runs_via_agent_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("hello.cmd");
        std::fs::write(&shim, "@echo shim-ok\r\n").unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let t = AgentTerminals::new(tx);
        let id = t.create(shim.to_str().unwrap(), &[], dir.path()).unwrap();
        t.wait_for_exit(id).await.unwrap();
        assert!(t.output(id).unwrap().0.contains("shim-ok"));
    }

    #[test]
    fn complete_prefix_holds_back_partial_char() {
        let b = "aé".as_bytes(); // 61 C3 A9
        assert_eq!(complete_prefix(&b[..2]), 1);
        assert_eq!(complete_prefix(b), 3);
        assert_eq!(complete_prefix(&[b'a', 0xFF, b'b']), 3);
    }
}
