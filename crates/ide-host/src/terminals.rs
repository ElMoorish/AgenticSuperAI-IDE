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
use tokio::sync::mpsc::{Sender, UnboundedSender};
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

/// Maximum agent terminals that may be running at once.
pub const MAX_LIVE_TERMINALS: usize = 16;
/// Maximum agent terminals kept (running + exited); the oldest exited are evicted.
pub const MAX_RETAINED_TERMINALS: usize = 32;
/// Capacity of the channel a `UserPty` sends events into.
pub const PTY_CHANNEL_CAPACITY: usize = 64;

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
    /// Too many live agent terminals (see `MAX_LIVE_TERMINALS`).
    TooMany,
    /// rows or cols was zero.
    InvalidSize,
}

impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalError::NotFound(p) => write!(f, "program not found: {p}"),
            TerminalError::Unknown => write!(f, "unknown terminal"),
            TerminalError::Io(e) => write!(f, "io error: {e}"),
            TerminalError::Pty(e) => write!(f, "pty error: {e}"),
            TerminalError::TooMany => write!(
                f,
                "too many running terminals (limit {MAX_LIVE_TERMINALS}); release one first"
            ),
            TerminalError::InvalidSize => write!(f, "terminal rows and cols must be non-zero"),
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

/// Extensions `CreateProcess` can launch (directly or via std's batch handling).
const LAUNCHABLE_EXTS: [&str; 4] = ["com", "exe", "bat", "cmd"];

fn has_launchable_ext(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| LAUNCHABLE_EXTS.iter().any(|l| l.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// PATHEXT filtered to what can actually be launched (so `.VBS`, `.JS`, `.MSC`
/// are ignored), in PATHEXT order; defaults to `.COM;.EXE;.BAT;.CMD`.
fn launchable_pathext(pathext: Option<&str>) -> Vec<String> {
    let default = ".COM;.EXE;.BAT;.CMD";
    let raw = pathext.unwrap_or(default);
    let mut v: Vec<String> = raw
        .split(';')
        .map(|e| e.trim())
        .filter(|e| {
            e.strip_prefix('.')
                .map(|x| LAUNCHABLE_EXTS.iter().any(|l| l.eq_ignore_ascii_case(x)))
                .unwrap_or(false)
        })
        .map(|e| e.to_string())
        .collect();
    if v.is_empty() {
        v = default.split(';').map(|e| e.to_string()).collect();
    }
    v
}

/// Resolves `cmd` to a launchable path.
///
/// - A name containing a path separator is a path: relative ones are resolved
///   against the REQUESTED `cwd` (not the host's), trying PATHEXT extensions
///   when it has no launchable extension of its own.
/// - A bare name is searched on PATH (or `path_override`). Only the extensions
///   com/exe/bat/cmd count as "already has an extension"; so `python3.12`
///   still gets PATHEXT appended.
pub(crate) fn resolve_program(
    cmd: &str,
    cwd: &Path,
    path_override: Option<OsString>,
) -> Result<PathBuf, TerminalError> {
    let pathext = std::env::var("PATHEXT").ok();
    resolve_program_with(cmd, cwd, path_override, pathext.as_deref())
}

pub(crate) fn resolve_program_with(
    cmd: &str,
    cwd: &Path,
    path_override: Option<OsString>,
    pathext: Option<&str>,
) -> Result<PathBuf, TerminalError> {
    let not_found = || TerminalError::NotFound(cmd.to_string());
    if cmd.is_empty() {
        return Err(not_found());
    }
    let exts: Vec<String> = if cfg!(windows) && !has_launchable_ext(cmd) {
        launchable_pathext(pathext)
    } else {
        Vec::new()
    };
    // Candidate file names for a base: the base itself if it is launchable as
    // is (or on non-Windows), plus base + each extension.
    let candidates = |base: PathBuf| -> Vec<PathBuf> {
        let mut v = Vec::new();
        if exts.is_empty() {
            v.push(base.clone());
        }
        for e in &exts {
            let mut s = base.clone().into_os_string();
            s.push(e);
            v.push(PathBuf::from(s));
        }
        v
    };
    if cmd.contains('/') || cmd.contains('\\') {
        let p = PathBuf::from(cmd);
        let p = if p.is_absolute() { p } else { cwd.join(p) };
        return candidates(p).into_iter().find(|c| c.is_file()).ok_or_else(not_found);
    }
    let path = path_override.or_else(|| std::env::var_os("PATH")).ok_or_else(not_found)?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = candidates(dir.join(cmd)).into_iter().find(|c| c.is_file()) {
            return Ok(found);
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

impl Entry {
    fn exited(&self) -> bool {
        self.exit.borrow().is_some()
    }
}

/// Agent-owned processes with piped output. Must be used inside a tokio runtime.
///
/// At most `MAX_LIVE_TERMINALS` run at once and at most
/// `MAX_RETAINED_TERMINALS` are remembered; beyond that the oldest exited
/// terminals are evicted (their ids become `Unknown`).
pub struct AgentTerminals {
    events: UnboundedSender<TerminalEvent>,
    next: AtomicU64,
    terms: Mutex<HashMap<TerminalId, Arc<Entry>>>,
}

const KILL_TIMEOUT: Duration = Duration::from_secs(5);
const READER_DRAIN: Duration = Duration::from_secs(2);

/// Kills the process tree with `taskkill /T /F` (resolved from
/// `%SystemRoot%\System32`), bounded by `KILL_TIMEOUT`.
#[cfg(windows)]
async fn kill_tree(pid: u32) -> Result<(), String> {
    let exe = match std::env::var_os("SystemRoot") {
        Some(r) => PathBuf::from(r).join("System32").join("taskkill.exe"),
        None => PathBuf::from("taskkill"),
    };
    let mut c = Command::new(exe);
    c.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x0800_0000);
    match tokio::time::timeout(KILL_TIMEOUT, c.status()).await {
        Ok(Ok(st)) if st.success() => Ok(()),
        Ok(Ok(st)) => Err(format!("taskkill exited with {st}")),
        Ok(Err(e)) => Err(format!("taskkill failed to run: {e}")),
        Err(_) => Err("taskkill timed out".into()),
    }
}
#[cfg(not(windows))]
async fn kill_tree(_pid: u32) -> Result<(), String> {
    Ok(())
}

/// Terminates the child (tree first), with every wait bounded. Returns the exit
/// code, or None if it could not be confirmed dead (logged).
async fn terminate(child: &mut tokio::process::Child, pid: Option<u32>) -> Option<i32> {
    if let Some(pid) = pid {
        if let Err(e) = kill_tree(pid).await {
            eprintln!("terminals: tree kill of pid {pid}: {e}");
        }
    }
    if let Ok(Ok(st)) = tokio::time::timeout(KILL_TIMEOUT, child.wait()).await {
        return st.code();
    }
    if let Err(e) = child.start_kill() {
        eprintln!("terminals: kill failed: {e}");
    }
    match tokio::time::timeout(KILL_TIMEOUT, child.wait()).await {
        Ok(Ok(st)) => st.code(),
        _ => {
            eprintln!("terminals: process did not exit after kill");
            None
        }
    }
}

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
        let mut terms = self.terms.lock().unwrap();
        if terms.values().filter(|e| !e.exited()).count() >= MAX_LIVE_TERMINALS {
            return Err(TerminalError::TooMany);
        }
        let program = resolve_program(cmd, cwd, None)?;
        let mut c = Command::new(&program);
        // SECURITY: for .bat/.cmd programs (npm.cmd, npx.cmd) we rely on the
        // standard library's batch-file argument escaping (the fix for
        // CVE-2024-24576, Rust 1.77.2+; see `rust-version`). Arguments the std
        // cannot escape safely make spawn fail with InvalidInput, surfaced here
        // as `TerminalError::Io`. We never build a shell command string.
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
                _ = kill_rx => terminate(&mut child, pid).await,
            };
            // One shared deadline for both readers; a grandchild holding a pipe
            // open must not delay Exited. Stragglers are aborted so nothing is
            // emitted after Exited.
            let deadline = Instant::now() + READER_DRAIN;
            for mut r in readers {
                if tokio::time::timeout_at(deadline, &mut r).await.is_err() {
                    r.abort();
                    let _ = r.await;
                }
            }
            let _ = events.send(TerminalEvent::Exited { id, code });
            let _ = exit_tx.send(Some(code));
        });

        let entry = Arc::new(Entry { ring, exit: exit_rx, kill: Mutex::new(Some(kill_tx)) });
        terms.insert(id, entry);
        while terms.len() > MAX_RETAINED_TERMINALS {
            let oldest = terms.iter().filter(|(_, e)| e.exited()).map(|(k, _)| *k).min();
            match oldest {
                Some(k) => {
                    terms.remove(&k);
                }
                None => break,
            }
        }
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
    /// forgets the terminal. Harmless if it already exited or is unknown.
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

/// Lossily decodes a prefix of `raw` into a String of at most `MAX_CHUNK`
/// bytes (invalid bytes become 3-byte U+FFFD, so the String can be larger than
/// the raw bytes). Returns the string and how many raw bytes it consumed.
fn take_chunk(raw: &[u8]) -> (String, usize) {
    let mut out = String::new();
    let mut used = 0;
    for ch in raw.utf8_chunks() {
        let v = ch.valid();
        if out.len() + v.len() > MAX_CHUNK {
            let mut k = MAX_CHUNK - out.len();
            while !v.is_char_boundary(k) {
                k -= 1;
            }
            out.push_str(&v[..k]);
            return (out, used + k);
        }
        out.push_str(v);
        used += v.len();
        let inv = ch.invalid();
        if !inv.is_empty() {
            if out.len() + 3 > MAX_CHUNK {
                return (out, used);
            }
            out.push('\u{FFFD}');
            used += inv.len();
        }
    }
    (out, used)
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
        let (chunk, used) = take_chunk(&pending[..cut]);
        if used == 0 {
            break;
        }
        pending.drain(..used);
        if emitted.load(Ordering::Relaxed) >= EMISSION_CAP {
            if !capped.swap(true, Ordering::SeqCst) {
                let _ = events.send(TerminalEvent::Truncated { id });
            }
            continue;
        }
        emitted.fetch_add(chunk.len(), Ordering::Relaxed);
        let _ = events.send(TerminalEvent::Output { id, chunk });
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
    /// Spawns `shell` on a pty. Events go through a bounded channel (create it
    /// with `mpsc::channel(PTY_CHANNEL_CAPACITY)`): the reader thread uses
    /// `blocking_send`, so a slow consumer backpressures the child exactly as a
    /// real terminal would. If the receiver is dropped the reader keeps
    /// draining and discarding so teardown cannot hang.
    pub fn spawn(
        shell: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        events: Sender<PtyEvent>,
    ) -> Result<UserPty, TerminalError> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        if rows == 0 || cols == 0 {
            return Err(TerminalError::InvalidSize);
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(pty_err)?;
        let program = resolve_program(shell, cwd, None)?;
        let mut cb = CommandBuilder::new(program);
        cb.cwd(cwd);
        let mut child = pair.slave.spawn_command(cb).map_err(pty_err)?;
        drop(pair.slave);
        let io = pair
            .master
            .try_clone_reader()
            .map_err(pty_err)
            .and_then(|r| pair.master.take_writer().map_err(pty_err).map(|w| (r, w)));
        let (mut reader, writer) = match io {
            Ok(x) => x,
            Err(e) => {
                let _ = child.kill();
                return Err(e);
            }
        };
        let killer = child.clone_killer();

        // Reader: blocking, forwards chunks of up to 8 KB, ends at EOF/error.
        let tx = events.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut open = true;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // Keep draining even if the receiver is gone: ConPTY
                        // teardown can block while its output is not being read.
                        if open && tx.blocking_send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            open = false;
                        }
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
                    let _ = events.blocking_send(PtyEvent::Exited(Some(st.exit_code() as i32)));
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = events.blocking_send(PtyEvent::Exited(None));
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
        if rows == 0 || cols == 0 {
            return Err(TerminalError::InvalidSize);
        }
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

    fn dir_os(d: &Path) -> Option<OsString> {
        Some(d.as_os_str().to_owned())
    }

    #[test]
    fn resolves_cmd_shim_on_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.cmd"), "@echo hi\r\n").unwrap();
        let p = resolve_program("hello", dir.path(), dir_os(dir.path())).unwrap();
        assert!(p.to_string_lossy().to_lowercase().ends_with("hello.cmd"), "{p:?}");
        assert!(p.starts_with(dir.path()));
    }

    #[test]
    fn unknown_program_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve_program("nope-nope", dir.path(), dir_os(dir.path()));
        assert!(matches!(r, Err(TerminalError::NotFound(_))));
    }

    #[test]
    fn resolves_dotted_name_like_python3_12() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("python3.12.exe"), b"").unwrap();
        let p = resolve_program("python3.12", dir.path(), dir_os(dir.path())).unwrap();
        assert!(p.to_string_lossy().to_lowercase().ends_with("python3.12.exe"), "{p:?}");
        // an explicit launchable extension is used as is
        let p = resolve_program("python3.12.exe", dir.path(), dir_os(dir.path())).unwrap();
        assert!(p.to_string_lossy().ends_with("python3.12.exe"));
    }

    #[test]
    fn ignores_vbs_in_pathext() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foo.vbs"), b"").unwrap();
        let r = resolve_program_with("foo", dir.path(), dir_os(dir.path()), Some(".VBS;.JS;.EXE"));
        assert!(matches!(r, Err(TerminalError::NotFound(_))), "{r:?}");
        std::fs::write(dir.path().join("foo.exe"), b"").unwrap();
        let p = resolve_program_with("foo", dir.path(), dir_os(dir.path()), Some(".VBS;.EXE"))
            .unwrap();
        assert!(p.to_string_lossy().to_lowercase().ends_with("foo.exe"));
    }

    #[test]
    fn relative_path_resolves_against_requested_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("tool.cmd"), "@echo x\r\n").unwrap();
        let p = resolve_program(".\\sub\\tool.cmd", dir.path(), None).unwrap();
        assert!(p.starts_with(dir.path()), "{p:?}");
        let p = resolve_program("sub/tool", dir.path(), None).unwrap();
        assert!(p.to_string_lossy().to_lowercase().ends_with("tool.cmd"), "{p:?}");
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

    #[test]
    fn take_chunk_bounds_lossy_expansion() {
        let raw = vec![0xFFu8; 40_000];
        let (s, used) = take_chunk(&raw[..MAX_CHUNK]);
        assert!(s.len() <= MAX_CHUNK);
        assert!(used > 0 && used < MAX_CHUNK);
        let (s, used) = take_chunk("é".repeat(MAX_CHUNK).as_bytes());
        assert!(s.len() <= MAX_CHUNK && used == s.len());
    }
}
