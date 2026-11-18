//! Application state and command logic.
//!
//! Every command is a plain method on [`AppState`] (or a free function) so it can be
//! tested without a Tauri runtime; `commands.rs` only adapts them to Tauri.
//!
//! Locking rules: all state lives behind `std::sync::Mutex` guards that are never held
//! across an `.await` (the one exception is `init_lock`, a tokio mutex whose only job is
//! to serialise agent initialisation). Every path that comes from the webview or from the
//! agent is resolved through `ide_host::Workspace` before it touches the disk.

use ide_host::acp::AgentEvent;
use ide_host::editor::{BufferView, EditorHandler, HandlerOutcome, PendingCommand, PendingEdit};
use ide_host::files::{read_stamped, write_atomic, FileStamp, WriteError};
use ide_host::probe::{self, DetectedServer, ProbeResult};
use ide_host::settings::Settings;
use ide_host::supervisor::{AgentConfig, EventRx};
use ide_host::terminals::{
    AgentTerminals, PtyEvent, TerminalError, TerminalEvent, UserPty, PTY_CHANNEL_CAPACITY,
};
use ide_host::watcher::{FsBatch, Watcher};
use ide_host::{AcpError, FsError, Supervisor, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// Event coalescing window and per-batch byte cap (plan: 16 ms / 64 KB).
pub const BATCH_INTERVAL: Duration = Duration::from_millis(16);
pub const BATCH_BYTES: usize = 64 * 1024;
/// Files above this size are refused by `read_file` (the UI also caps at 5 MB).
pub const MAX_READ_BYTES: u64 = 50 * 1024 * 1024;
const MAX_DIR_ENTRIES: usize = 20_000;
const MAX_USER_TERMINALS: usize = 8;
const GPU_INTERVAL: Duration = Duration::from_secs(2);
const INIT_TIMEOUT: Duration = Duration::from_secs(15);

// ------------------------------------------------------------------ errors

/// Error returned to the webview as `{code, message}`.
///
/// Codes: `no_workspace`, `escapes`, `not_found`, `io`, `conflict`, `too_large`,
/// `bad_request`, `busy`, `agent_missing`, `agent`, `unknown_approval`, `terminal`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        AppError { code: code.into(), message: message.into() }
    }
    pub fn no_workspace() -> Self {
        AppError::new("no_workspace", "no folder is open")
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        AppError::new("bad_request", m)
    }
}

impl From<FsError> for AppError {
    fn from(e: FsError) -> Self {
        let code = match &e {
            FsError::Escapes(_) => "escapes",
            FsError::NotFound(_) => "not_found",
            FsError::Io(_) => "io",
        };
        AppError::new(code, e.to_string())
    }
}

impl From<WriteError> for AppError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::Conflict => AppError::new("conflict", e.to_string()),
            WriteError::Fs(f) => f.into(),
        }
    }
}

impl From<TerminalError> for AppError {
    fn from(e: TerminalError) -> Self {
        AppError::new("terminal", e.to_string())
    }
}

impl From<AcpError> for AppError {
    fn from(e: AcpError) -> Self {
        AppError::new("agent", e.to_string())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

// ------------------------------------------------------------------ events

/// Events streamed to the webview. `type` is the tag; variant names and fields are camelCase.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UiEvent {
    Chunk {
        text: String,
    },
    ToolCall {
        id: String,
        title: String,
        kind: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    ToolUpdate {
        id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
        tokens_per_second: f64,
    },
    /// `kind` is `"edit"` (title = workspace-relative path, old/new set) or
    /// `"command"` (title = tool title, command set).
    Approval {
        id: u64,
        kind: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        old: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        new: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// Reserved for Task 11 (UI applies an accepted edit itself). Not emitted yet.
    #[allow(dead_code)]
    ApplyEdit {
        id: u64,
        rel: String,
        new_text: String,
    },
    /// `state`: `running` | `crashed` | `restarted` | `gave_up`.
    AgentState {
        state: String,
    },
    TurnEnd {
        stop_reason: String,
    },
    FsChanged {
        paths: Vec<String>,
        overflow: bool,
    },
    Gpu {
        #[serde(rename = "usedMiB")]
        used_mib: u32,
        #[serde(rename = "totalMiB")]
        total_mib: u32,
    },
    AgentTerminalOutput {
        id: u64,
        chunk: String,
    },
    AgentTerminalTruncated {
        id: u64,
    },
    AgentTerminalExit {
        id: u64,
        code: Option<i32>,
    },
    UserTerminalExit {
        id: u64,
        code: Option<i32>,
    },
}

impl UiEvent {
    /// Approximate payload size in bytes, used for the batch cap.
    pub fn weight(&self) -> usize {
        const BASE: usize = 48;
        let opt = |o: &Option<String>| o.as_ref().map_or(0, |s| s.len());
        BASE + match self {
            UiEvent::Chunk { text } => return text.len(),
            UiEvent::ToolCall { id, title, kind, status, content } => {
                id.len() + title.len() + kind.len() + status.len() + opt(content)
            }
            UiEvent::ToolUpdate { id, status, content } => id.len() + status.len() + opt(content),
            UiEvent::Approval { kind, title, old, new, command, .. } => {
                kind.len() + title.len() + opt(old) + opt(new) + opt(command)
            }
            UiEvent::ApplyEdit { rel, new_text, .. } => rel.len() + new_text.len(),
            UiEvent::FsChanged { paths, .. } => paths.iter().map(|p| p.len() + 4).sum(),
            UiEvent::AgentTerminalOutput { chunk, .. } => chunk.len(),
            UiEvent::AgentState { state } => state.len(),
            UiEvent::TurnEnd { stop_reason } => stop_reason.len(),
            _ => 0,
        }
    }
}

/// Merges adjacent `Chunk` text and groups events into batches of at most `cap` bytes.
///
/// Rules: only *adjacent* chunks merge (order is never changed); every non-chunk event is
/// kept exactly as pushed; a chunk bigger than the cap is split at a char boundary, never
/// dropped; an indivisible non-chunk event larger than the cap gets a batch of its own.
pub struct Coalescer {
    cap: usize,
    sealed: Vec<Vec<UiEvent>>,
    cur: Vec<UiEvent>,
    cur_bytes: usize,
}

impl Coalescer {
    pub fn new(cap: usize) -> Self {
        Coalescer { cap: cap.max(4), sealed: Vec::new(), cur: Vec::new(), cur_bytes: 0 }
    }

    pub fn push(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::Chunk { text } => self.push_text(text),
            other => {
                let w = other.weight();
                if !self.cur.is_empty() && self.cur_bytes + w > self.cap {
                    self.seal();
                }
                self.cur_bytes += w;
                self.cur.push(other);
            }
        }
    }

    fn push_text(&mut self, mut text: String) {
        while !text.is_empty() {
            let room = self.cap.saturating_sub(self.cur_bytes);
            if text.len() <= room {
                self.append(text);
                return;
            }
            let mut at = room.min(text.len());
            while at > 0 && !text.is_char_boundary(at) {
                at -= 1;
            }
            if at == 0 {
                if self.cur.is_empty() {
                    // cap smaller than one char: emit that single char
                    at = text.chars().next().map_or(0, char::len_utf8);
                } else {
                    self.seal();
                    continue;
                }
            }
            let rest = text.split_off(at);
            self.append(text);
            self.seal();
            text = rest;
        }
    }

    fn append(&mut self, t: String) {
        self.cur_bytes += t.len();
        if let Some(UiEvent::Chunk { text }) = self.cur.last_mut() {
            text.push_str(&t);
        } else {
            self.cur.push(UiEvent::Chunk { text: t });
        }
    }

    fn seal(&mut self) {
        if !self.cur.is_empty() {
            self.sealed.push(std::mem::take(&mut self.cur));
        }
        self.cur_bytes = 0;
    }

    /// True once a full batch is ready: flush now instead of waiting for the timer.
    pub fn is_full(&self) -> bool {
        !self.sealed.is_empty() || self.cur_bytes >= self.cap
    }

    /// Drain everything, in order, as batches each within the cap.
    pub fn take(&mut self) -> Vec<Vec<UiEvent>> {
        self.seal();
        std::mem::take(&mut self.sealed)
    }
}

/// Delivers one event to the webview; returns false when the listener is gone.
pub type Sink = Arc<dyn Fn(&UiEvent) -> bool + Send + Sync>;
pub type SinkSlot = Arc<Mutex<Option<Sink>>>;

fn flush(co: &mut Coalescer, slot: &SinkSlot) {
    let batches = co.take();
    let sink = slot.lock().unwrap().clone();
    let Some(sink) = sink else { return }; // no window listening: drop
    for ev in batches.iter().flatten() {
        if !sink(ev) {
            // listener closed: forget it (unless replaced meanwhile)
            let mut g = slot.lock().unwrap();
            if g.as_ref().is_some_and(|cur| Arc::ptr_eq(cur, &sink)) {
                *g = None;
            }
            return;
        }
    }
}

/// Reads `rx`, coalesces for [`BATCH_INTERVAL`] / [`BATCH_BYTES`] and hands events to the sink.
pub async fn run_pump(mut rx: UnboundedReceiver<UiEvent>, slot: SinkSlot) {
    let mut co = Coalescer::new(BATCH_BYTES);
    while let Some(first) = rx.recv().await {
        co.push(first);
        let deadline = tokio::time::sleep(BATCH_INTERVAL);
        tokio::pin!(deadline);
        while !co.is_full() {
            tokio::select! {
                biased;
                ev = rx.recv() => match ev {
                    Some(e) => co.push(e),
                    None => break,
                },
                _ = &mut deadline => break,
            }
        }
        flush(&mut co, &slot);
    }
    flush(&mut co, &slot);
}

// ------------------------------------------------------------------ DTOs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StampDto {
    pub mtime_ms: u64,
    /// 16 hex digits (a JS number cannot carry 64 bits).
    pub hash: String,
}

impl From<&FileStamp> for StampDto {
    fn from(s: &FileStamp) -> Self {
        StampDto {
            mtime_ms: s.mtime.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            hash: format!("{:016x}", s.hash),
        }
    }
}

impl StampDto {
    fn to_stamp(&self) -> Result<FileStamp, AppError> {
        let hash = u64::from_str_radix(&self.hash, 16)
            .map_err(|_| AppError::bad_request("invalid stamp hash"))?;
        Ok(FileStamp { mtime: UNIX_EPOCH + Duration::from_millis(self.mtime_ms), hash })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    /// Workspace-relative, forward slashes.
    pub rel: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReadFileResult {
    pub text: String,
    pub stamp: StampDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProbeDto {
    pub ok: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

impl From<ProbeResult> for ProbeDto {
    fn from(p: ProbeResult) -> Self {
        ProbeDto { ok: p.ok, models: p.models, error: p.error }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DetectedDto {
    pub name: String,
    pub url: String,
}

impl From<DetectedServer> for DetectedDto {
    fn from(d: DetectedServer) -> Self {
        DetectedDto { name: d.name, url: d.url }
    }
}

// ------------------------------------------------------------------ pure helpers

fn rel_string(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Find the agent binary: `ASA_AGENT_EXE` override, then next to the app exe (installed
/// sidecar), then `target/{debug,release}/agent-core.exe` of any ancestor (dev builds).
pub fn locate_agent_exe(current_exe: &Path, env_override: Option<&str>) -> Option<PathBuf> {
    let name = format!("agent-core{}", std::env::consts::EXE_SUFFIX);
    if let Some(o) = env_override.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(o);
        if p.is_file() {
            return Some(p);
        }
    }
    let dir = current_exe.parent()?;
    let sibling = dir.join(&name);
    if sibling.is_file() {
        return Some(sibling);
    }
    for anc in dir.ancestors() {
        for profile in ["debug", "release"] {
            let c = anc.join("target").join(profile).join(&name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Map an agent `session/update` payload to a UI event (unknown kinds are dropped).
pub fn map_update(update: &Value) -> Option<UiEvent> {
    let s = |k: &str| update.get(k).and_then(Value::as_str).map(str::to_string);
    match update.get("sessionUpdate")?.as_str()? {
        "agent_message_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?.to_string();
            Some(UiEvent::Chunk { text })
        }
        "tool_call" => Some(UiEvent::ToolCall {
            id: s("toolCallId")?,
            title: s("title").unwrap_or_default(),
            kind: s("kind").unwrap_or_else(|| "other".into()),
            status: s("status").unwrap_or_else(|| "pending".into()),
            content: s("content"),
        }),
        "tool_call_update" => Some(UiEvent::ToolUpdate {
            id: s("toolCallId")?,
            status: s("status").unwrap_or_default(),
            content: s("content"),
        }),
        "usage" => Some(UiEvent::Usage {
            prompt_tokens: update.get("promptTokens").and_then(Value::as_u64).unwrap_or(0),
            completion_tokens: update.get("completionTokens").and_then(Value::as_u64).unwrap_or(0),
            tokens_per_second: update.get("tokensPerSecond").and_then(Value::as_f64).unwrap_or(0.0),
        }),
        _ => None,
    }
}

fn valid_endpoint(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && l.len() > 8 && !e.contains(char::is_whitespace)
}

/// Validate and normalise settings coming from the webview.
pub fn sanitize_settings(mut s: Settings, current: &Settings) -> Result<Settings, AppError> {
    s.endpoint = s.endpoint.trim().trim_end_matches('/').to_string();
    if !valid_endpoint(&s.endpoint) {
        return Err(AppError::bad_request("endpoint must be an http:// or https:// URL"));
    }
    s.model = s.model.trim().to_string();
    s.theme = s.theme.trim().to_string();
    if s.theme.is_empty() {
        s.theme = current.theme.clone();
    }
    s.font_size = s.font_size.clamp(8, 32);
    let mut ignore: Vec<String> = Vec::new();
    for g in s.ignore.drain(..) {
        let g = g.trim().to_string();
        if !g.is_empty() && g.len() <= 128 && !ignore.contains(&g) && ignore.len() < 200 {
            ignore.push(g);
        }
    }
    s.ignore = ignore;
    s.recent = current.recent.clone(); // owned by the host (open_folder)
    Ok(s)
}

// ------------------------------------------------------------------ workspace context

/// The UI's unsaved buffers are not visible to the host yet (a later task can add a
/// sync command); the agent reads disk content.
pub struct NoBuffers;
impl BufferView for NoBuffers {
    fn open_buffer(&self, _abs: &Path) -> Option<String> {
        None
    }
}

enum Pending {
    Edit { pe: PendingEdit, stamp: Option<FileStamp> },
    Command(PendingCommand),
}

struct AgentCtx {
    sup: Arc<Supervisor>,
    handler: Arc<EditorHandler<NoBuffers>>,
    terms: Arc<AgentTerminals>,
    live_terms: Mutex<HashSet<u64>>,
    /// Bumped on every Exited/Restarted: a session id from an older epoch is stale.
    epoch: AtomicU64,
    session: Mutex<Option<(u64, String)>>,
    init_lock: tokio::sync::Mutex<()>,
    /// Ordered injection into the agent event loop (see `run_turn`).
    ordered_tx: UnboundedSender<UiEvent>,
}

pub struct WsCtx {
    ws: Workspace,
    ui_tx: UnboundedSender<UiEvent>,
    pending: Mutex<HashMap<u64, Pending>>,
    agent: Option<Arc<AgentCtx>>,
    watcher: Mutex<Option<Watcher>>,
    watch_task: Mutex<Option<JoinHandle<()>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl WsCtx {
    fn send(&self, ev: UiEvent) {
        let _ = self.ui_tx.send(ev);
    }

    fn start_watcher(&self, ignore: &[String]) {
        match Watcher::start(&self.ws, ignore) {
            Ok((w, mut rx)) => {
                let tx = self.ui_tx.clone();
                let task = tokio::spawn(async move {
                    while let Some(FsBatch { changes, overflow }) = rx.recv().await {
                        let mut paths: Vec<String> = changes.into_iter().map(|c| c.path).collect();
                        paths.sort();
                        paths.dedup();
                        if tx.send(UiEvent::FsChanged { paths, overflow }).is_err() {
                            break;
                        }
                    }
                });
                if let Some(old) = self.watch_task.lock().unwrap().replace(task) {
                    old.abort();
                }
                *self.watcher.lock().unwrap() = Some(w);
            }
            Err(e) => eprintln!("watcher failed to start: {e}"),
        }
    }

    async fn shutdown(&self) {
        for t in self.tasks.lock().unwrap().drain(..) {
            t.abort();
        }
        if let Some(t) = self.watch_task.lock().unwrap().take() {
            t.abort();
        }
        *self.watcher.lock().unwrap() = None;
        self.pending.lock().unwrap().clear();
        if let Some(a) = &self.agent {
            let ids: Vec<u64> = a.live_terms.lock().unwrap().drain().collect();
            for id in ids {
                a.terms.release(id);
            }
            a.sup.shutdown().await;
        }
    }

    // ---- agent events

    fn on_agent_event(self: &Arc<Self>, agent: &Arc<AgentCtx>, ev: AgentEvent) {
        match ev {
            AgentEvent::Update { update, .. } => {
                if let Some(u) = map_update(&update) {
                    self.send(u);
                }
            }
            AgentEvent::Request { id, method, params } => self.on_request(agent, id, method, params),
            AgentEvent::Exited { .. } => {
                agent.epoch.fetch_add(1, Ordering::SeqCst);
                self.pending.lock().unwrap().clear();
                self.send(UiEvent::AgentState { state: "crashed".into() });
            }
            AgentEvent::Restarted { .. } => {
                agent.epoch.fetch_add(1, Ordering::SeqCst);
                self.pending.lock().unwrap().clear();
                self.send(UiEvent::AgentState { state: "restarted".into() });
            }
            AgentEvent::GaveUp => self.send(UiEvent::AgentState { state: "gave_up".into() }),
        }
    }

    fn on_request(self: &Arc<Self>, agent: &Arc<AgentCtx>, id: Value, method: String, params: Value) {
        let this = self.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            if method.starts_with("terminal/") {
                this.terminal_request(&agent, id, &method, &params).await;
                return;
            }
            let handler = agent.handler.clone();
            let (id2, m2) = (id.clone(), method.clone());
            let out = tokio::task::spawn_blocking(move || handler.handle(id2, &m2, &params)).await;
            match out {
                Ok(HandlerOutcome::Reply(v)) => agent.sup.respond(id, Ok(v)),
                Ok(HandlerOutcome::Error(e)) => agent.sup.respond(id, Err(e)),
                Ok(HandlerOutcome::NeedsApproval(pe)) => {
                    let stamp = read_stamped(&this.ws, &pe.path).ok().map(|(_, s)| s);
                    let ev = UiEvent::Approval {
                        id: pe.id,
                        kind: "edit".into(),
                        title: pe.path.clone(),
                        old: Some(pe.old.clone()),
                        new: Some(pe.new.clone()),
                        command: None,
                    };
                    this.pending.lock().unwrap().insert(pe.id, Pending::Edit { pe, stamp });
                    this.send(ev);
                }
                Ok(HandlerOutcome::NeedsPermission(pc)) => {
                    let ev = UiEvent::Approval {
                        id: pc.id,
                        kind: "command".into(),
                        title: pc.title.clone(),
                        old: None,
                        new: None,
                        command: Some(pc.command.clone()),
                    };
                    this.pending.lock().unwrap().insert(pc.id, Pending::Command(pc));
                    this.send(ev);
                }
                Err(e) => agent.sup.respond(
                    id,
                    Err(json!({"code": -32603, "message": format!("internal error: {e}")})),
                ),
            }
        });
    }

    async fn terminal_request(&self, agent: &Arc<AgentCtx>, id: Value, method: &str, params: &Value) {
        let err = |code: i64, m: String| Err(json!({"code": code, "message": m}));
        let tid = params.get("terminalId").and_then(Value::as_u64);
        let result: Result<Value, Value> = match method {
            "terminal/create" => match params.get("command").and_then(Value::as_str) {
                None => err(-32602, "missing string param: command".into()),
                Some(cmd) => {
                    let args: Vec<String> = params
                        .get("args")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
                    match self.ws.resolve(cwd) {
                        Err(e) => err(-32000, e.to_string()),
                        Ok(cwd) => match agent.terms.create(cmd, &args, &cwd) {
                            Ok(t) => {
                                agent.live_terms.lock().unwrap().insert(t);
                                Ok(json!({ "terminalId": t }))
                            }
                            Err(e) => err(-32000, e.to_string()),
                        },
                    }
                }
            },
            "terminal/wait_for_exit" => match tid {
                None => err(-32602, "missing terminalId".into()),
                Some(t) => match agent.terms.wait_for_exit(t).await {
                    Ok(st) => Ok(json!({ "exitCode": st.code })),
                    Err(e) => err(-32000, e.to_string()),
                },
            },
            "terminal/output" => match tid {
                None => err(-32602, "missing terminalId".into()),
                Some(t) => match agent.terms.output(t) {
                    Ok((output, truncated)) => Ok(json!({ "output": output, "truncated": truncated })),
                    Err(e) => err(-32000, e.to_string()),
                },
            },
            "terminal/release" => match tid {
                None => err(-32602, "missing terminalId".into()),
                Some(t) => {
                    agent.live_terms.lock().unwrap().remove(&t);
                    agent.terms.release(t);
                    Ok(json!({}))
                }
            },
            m => err(-32601, format!("method not found: {m}")),
        };
        agent.sup.respond(id, result);
    }

    // ---- turns

    async fn ensure_session(&self, agent: &Arc<AgentCtx>) -> Result<String, AppError> {
        let _g = agent.init_lock.lock().await;
        let ep = agent.epoch.load(Ordering::SeqCst);
        if let Some((e, id)) = agent.session.lock().unwrap().clone() {
            if e == ep {
                return Ok(id);
            }
        }
        let call = |m: &'static str, p: Value| {
            let sup = agent.sup.clone();
            async move {
                match tokio::time::timeout(INIT_TIMEOUT, sup.request(m, p)).await {
                    Ok(r) => r.map_err(AppError::from),
                    Err(_) => Err(AppError::new("agent", format!("agent did not answer {m} in time"))),
                }
            }
        };
        call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {
                "fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true}}),
        )
        .await?;
        let r = call(
            "session/new",
            json!({"cwd": self.ws.root().to_string_lossy(), "mcpServers": []}),
        )
        .await?;
        let sid = r
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::new("agent", "session/new returned no sessionId"))?
            .to_string();
        *agent.session.lock().unwrap() = Some((ep, sid.clone()));
        self.send(UiEvent::AgentState { state: "running".into() });
        Ok(sid)
    }

    async fn run_turn(&self, agent: &Arc<AgentCtx>, mode: &str, text: &str) -> Result<String, AppError> {
        let sid = self.ensure_session(agent).await?;
        let params = json!({"sessionId": sid, "mode": mode, "prompt": [{"type": "text", "text": text}]});
        match agent.sup.request("session/prompt", params).await {
            Ok(v) => Ok(v.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_string()),
            Err(AcpError::Remote(v)) => Err(AppError::new(
                "agent",
                v.get("message").and_then(Value::as_str).unwrap_or("agent error").to_string(),
            )),
            Err(e) => Err(e.into()),
        }
    }
}

/// Clears the busy flag when the turn ends, however it ends.
struct TurnGuard(Arc<AtomicBool>);
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------ AppState

struct Inner {
    ws: Option<Arc<WsCtx>>,
    settings: Settings,
}

pub struct AppState {
    settings_dir: PathBuf,
    agent_exe: Option<PathBuf>,
    ui_tx: UnboundedSender<UiEvent>,
    ui_rx: Mutex<Option<UnboundedReceiver<UiEvent>>>,
    sink: SinkSlot,
    inner: Mutex<Inner>,
    open_lock: tokio::sync::Mutex<()>,
    turn_active: Arc<AtomicBool>,
    terms: Arc<Mutex<HashMap<u64, Arc<UserPty>>>>,
    next_term: AtomicU64,
}

impl AppState {
    pub fn new(settings_dir: PathBuf, agent_exe: Option<PathBuf>) -> AppState {
        let settings = Settings::load(&settings_dir);
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        AppState {
            settings_dir,
            agent_exe,
            ui_tx,
            ui_rx: Mutex::new(Some(ui_rx)),
            sink: Arc::new(Mutex::new(None)),
            inner: Mutex::new(Inner { ws: None, settings }),
            open_lock: tokio::sync::Mutex::new(()),
            turn_active: Arc::new(AtomicBool::new(false)),
            terms: Arc::new(Mutex::new(HashMap::new())),
            next_term: AtomicU64::new(1),
        }
    }

    /// Receiver of every UI event; taken once by the pump task.
    pub fn take_event_rx(&self) -> Option<UnboundedReceiver<UiEvent>> {
        self.ui_rx.lock().unwrap().take()
    }

    pub fn sink_slot(&self) -> SinkSlot {
        self.sink.clone()
    }

    /// Install the (single) webview listener; later calls replace it.
    pub fn set_sink(&self, sink: Sink) {
        *self.sink.lock().unwrap() = Some(sink);
    }

    pub fn has_sink(&self) -> bool {
        self.sink.lock().unwrap().is_some()
    }

    pub fn emit(&self, ev: UiEvent) {
        let _ = self.ui_tx.send(ev);
    }

    fn ctx(&self) -> Result<Arc<WsCtx>, AppError> {
        self.inner.lock().unwrap().ws.clone().ok_or_else(AppError::no_workspace)
    }

    fn workspace(&self) -> Result<Workspace, AppError> {
        Ok(self.ctx()?.ws.clone())
    }

    // ---- workspace

    pub async fn open_folder(&self, path: &str) -> Result<WorkspaceInfo, AppError> {
        let _g = self.open_lock.lock().await;
        self.open_inner(path).await
    }

    async fn open_inner(&self, path: &str) -> Result<WorkspaceInfo, AppError> {
        let ws = Workspace::open(Path::new(path))?;
        let old = self.inner.lock().unwrap().ws.take();
        if let Some(old) = old {
            old.shutdown().await;
        }
        self.turn_active.store(false, Ordering::SeqCst);
        let settings = self.inner.lock().unwrap().settings.clone();

        let (ordered_tx, mut ordered_rx) = mpsc::unbounded_channel::<UiEvent>();
        let mut tasks: Vec<JoinHandle<()>> = Vec::new();
        let mut agent_parts: Option<(Arc<AgentCtx>, EventRx, UnboundedReceiver<TerminalEvent>)> = None;
        if let Some(exe) = &self.agent_exe {
            let mut cfg = AgentConfig::new(exe.clone(), ws.root().to_path_buf());
            cfg.env.push(("ASA_LLM_URL".into(), settings.endpoint.clone()));
            if !settings.model.is_empty() {
                cfg.env.push(("ASA_LLM_MODEL".into(), settings.model.clone()));
            }
            let (sup, events) = Supervisor::start(cfg);
            let (tev_tx, tev_rx) = mpsc::unbounded_channel();
            agent_parts = Some((
                Arc::new(AgentCtx {
                    sup: Arc::new(sup),
                    handler: Arc::new(EditorHandler::new(ws.clone(), NoBuffers)),
                    terms: Arc::new(AgentTerminals::new(tev_tx)),
                    live_terms: Mutex::new(HashSet::new()),
                    epoch: AtomicU64::new(0),
                    session: Mutex::new(None),
                    init_lock: tokio::sync::Mutex::new(()),
                    ordered_tx,
                }),
                events,
                tev_rx,
            ));
        }
        let ctx = Arc::new(WsCtx {
            ws: ws.clone(),
            ui_tx: self.ui_tx.clone(),
            pending: Mutex::new(HashMap::new()),
            agent: agent_parts.as_ref().map(|p| p.0.clone()),
            watcher: Mutex::new(None),
            watch_task: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
        });
        if let Some((agent, mut events, mut tev_rx)) = agent_parts {
            // Agent event loop. `ordered_rx` is polled only when `events` is empty, so a
            // TurnEnd injected after the prompt response always follows every update the
            // agent sent before that response.
            let c = ctx.clone();
            let a = agent.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        ev = events.recv() => match ev {
                            Some(ev) => c.on_agent_event(&a, ev),
                            None => break,
                        },
                        Some(ev) = ordered_rx.recv() => c.send(ev),
                    }
                }
            }));
            let tx = self.ui_tx.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(ev) = tev_rx.recv().await {
                    let ui = match ev {
                        TerminalEvent::Output { id, chunk } => UiEvent::AgentTerminalOutput { id, chunk },
                        TerminalEvent::Truncated { id } => UiEvent::AgentTerminalTruncated { id },
                        TerminalEvent::Exited { id, code } => UiEvent::AgentTerminalExit { id, code },
                    };
                    if tx.send(ui).is_err() {
                        break;
                    }
                }
            }));
        }
        ctx.start_watcher(&settings.ignore);
        *ctx.tasks.lock().unwrap() = tasks;

        let info = WorkspaceInfo {
            root: ws.root().to_string_lossy().into_owned(),
            name: ws
                .root()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| ws.root().to_string_lossy().into_owned()),
        };
        let snapshot = {
            let mut g = self.inner.lock().unwrap();
            g.settings.push_recent(&info.root);
            g.ws = Some(ctx);
            g.settings.clone()
        };
        let dir = self.settings_dir.clone();
        let _ = tokio::task::spawn_blocking(move || snapshot.save(&dir)).await;
        Ok(info)
    }

    pub async fn shutdown(&self) {
        let _g = self.open_lock.lock().await;
        let old = self.inner.lock().unwrap().ws.take();
        if let Some(old) = old {
            old.shutdown().await;
        }
        let ptys: Vec<Arc<UserPty>> = self.terms.lock().unwrap().drain().map(|(_, p)| p).collect();
        for p in ptys {
            p.kill();
        }
    }

    // ---- files

    pub fn list_dir(&self, rel: &str) -> Result<Vec<DirEntryDto>, AppError> {
        let ws = self.workspace()?;
        let abs = ws.resolve(rel)?;
        let base = rel_string(ws.root(), &abs);
        let rd = std::fs::read_dir(&abs).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::new("not_found", format!("not found: {rel}"))
            } else {
                AppError::new("io", format!("cannot list {rel}: {e}"))
            }
        })?;
        let mut out: Vec<DirEntryDto> = Vec::new();
        for ent in rd.flatten().take(MAX_DIR_ENTRIES) {
            let name = ent.file_name().to_string_lossy().into_owned();
            let md = ent.metadata().ok();
            let is_dir = ent.path().is_dir();
            out.push(DirEntryDto {
                rel: if base.is_empty() { name.clone() } else { format!("{base}/{name}") },
                name,
                is_dir,
                size: if is_dir { None } else { md.map(|m| m.len()) },
            });
        }
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    pub fn read_file(&self, rel: &str) -> Result<ReadFileResult, AppError> {
        let ws = self.workspace()?;
        let abs = ws.resolve(rel)?;
        if let Ok(md) = std::fs::metadata(&abs) {
            if md.len() > MAX_READ_BYTES {
                return Err(AppError::new("too_large", format!("{rel} is {} bytes (limit {MAX_READ_BYTES})", md.len())));
            }
        }
        let (text, stamp) = read_stamped(&ws, rel)?;
        Ok(ReadFileResult { text, stamp: (&stamp).into() })
    }

    pub fn save_file(&self, rel: &str, text: &str, stamp: &StampDto) -> Result<StampDto, AppError> {
        let ws = self.workspace()?;
        ws.resolve(rel)?; // reject escapes before anything else (incl. a bad stamp)
        let expected = stamp.to_stamp()?;
        let new = write_atomic(&ws, rel, text, Some(&expected))?;
        Ok((&new).into())
    }

    // ---- agent

    pub async fn agent_prompt(&self, mode: &str, text: &str) -> Result<(), AppError> {
        if mode != "plan" && mode != "build" {
            return Err(AppError::bad_request("mode must be \"plan\" or \"build\""));
        }
        let ctx = self.ctx()?;
        let agent = ctx.agent.clone().ok_or_else(|| {
            AppError::new("agent_missing", "agent-core executable not found (build it or set ASA_AGENT_EXE)")
        })?;
        if self.turn_active.swap(true, Ordering::SeqCst) {
            return Err(AppError::new("busy", "a turn is already running"));
        }
        let _guard = TurnGuard(self.turn_active.clone());
        let result = ctx.run_turn(&agent, mode, text).await;
        let stop = match &result {
            Ok(s) => s.clone(),
            Err(_) => "error".to_string(),
        };
        let _ = agent.ordered_tx.send(UiEvent::TurnEnd { stop_reason: stop });
        result.map(|_| ())
    }

    pub fn agent_cancel(&self) {
        let Ok(ctx) = self.ctx() else { return };
        // The agent will fail the pending permission call itself; forget our copies.
        ctx.pending.lock().unwrap().clear();
        if let Some(a) = &ctx.agent {
            if let Some((_, sid)) = a.session.lock().unwrap().clone() {
                a.sup.notify("session/cancel", json!({ "sessionId": sid }));
            }
        }
    }

    /// Resolve a pending approval. Edits are written by the host for now (stamp-checked,
    /// atomic); Task 11 replaces this with the `ApplyEdit` / `confirm_edit` round trip.
    pub async fn resolve_approval(&self, id: u64, accept: bool) -> Result<(), AppError> {
        let ctx = self.ctx()?;
        let agent = ctx.agent.clone().ok_or_else(|| AppError::new("agent_missing", "no agent"))?;
        let item = ctx.pending.lock().unwrap().remove(&id);
        let Some(item) = item else {
            return Err(AppError::new("unknown_approval", format!("no pending approval {id}")));
        };
        match item {
            Pending::Command(pc) => {
                let option = if accept { "allow" } else { "reject" };
                agent.sup.respond(
                    pc.agent_id,
                    Ok(json!({"outcome": {"outcome": "selected", "optionId": option}})),
                );
                Ok(())
            }
            Pending::Edit { pe, stamp } => {
                if !accept {
                    agent.sup.respond(
                        pe.agent_id,
                        Err(json!({"code": -32000, "message": "edit rejected by the user"})),
                    );
                    return Ok(());
                }
                let ws = ctx.ws.clone();
                let (path, new) = (pe.path.clone(), pe.new.clone());
                let res = tokio::task::spawn_blocking(move || write_atomic(&ws, &path, &new, stamp.as_ref()))
                    .await
                    .map_err(|e| AppError::new("io", e.to_string()))?;
                match res {
                    Ok(_) => {
                        agent.sup.respond(pe.agent_id, Ok(Value::Null));
                        Ok(())
                    }
                    Err(e) => {
                        let err: AppError = e.into();
                        agent.sup.respond(
                            pe.agent_id,
                            Err(json!({"code": -32000, "message": err.message})),
                        );
                        Err(err)
                    }
                }
            }
        }
    }

    // ---- settings and probes

    pub fn get_settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }

    pub async fn set_settings(&self, s: Settings) -> Result<Settings, AppError> {
        let current = self.get_settings();
        let new = sanitize_settings(s, &current)?;
        let dir = self.settings_dir.clone();
        let to_save = new.clone();
        tokio::task::spawn_blocking(move || to_save.save(&dir))
            .await
            .map_err(|e| AppError::new("io", e.to_string()))?
            .map_err(|e| AppError::new("io", format!("cannot save settings: {e}")))?;
        let ctx = {
            let mut g = self.inner.lock().unwrap();
            g.settings = new.clone();
            g.ws.clone()
        };
        if let Some(ctx) = ctx {
            if new.ignore != current.ignore {
                ctx.start_watcher(&new.ignore);
            }
            let agent_env_changed = new.endpoint != current.endpoint || new.model != current.model;
            if agent_env_changed && ctx.agent.is_some() && !self.turn_active.load(Ordering::SeqCst) {
                // The agent reads its endpoint/model from the environment at spawn: restart it.
                let _g = self.open_lock.lock().await;
                let root = ctx.ws.root().to_string_lossy().into_owned();
                drop(ctx);
                let _ = self.open_inner(&root).await;
            }
        }
        Ok(new)
    }

    pub async fn probe_endpoint(&self, url: &str) -> Result<ProbeDto, AppError> {
        let url = url.trim().trim_end_matches('/').to_string();
        if !valid_endpoint(&url) {
            return Err(AppError::bad_request("endpoint must be an http:// or https:// URL"));
        }
        tokio::task::spawn_blocking(move || probe::probe_endpoint(&url).into())
            .await
            .map_err(|e| AppError::new("io", e.to_string()))
    }

    pub async fn detect_servers(&self) -> Result<Vec<DetectedDto>, AppError> {
        tokio::task::spawn_blocking(|| probe::detect_servers().into_iter().map(Into::into).collect())
            .await
            .map_err(|e| AppError::new("io", e.to_string()))
    }

    // ---- user terminals

    /// Start the user's shell on a pty. `out` receives raw output bytes (return false when
    /// the listener is gone). Must be called inside the tokio runtime.
    pub fn start_user_terminal(
        &self,
        rows: u16,
        cols: u16,
        out: Box<dyn Fn(Vec<u8>) -> bool + Send + Sync>,
    ) -> Result<u64, AppError> {
        if self.terms.lock().unwrap().len() >= MAX_USER_TERMINALS {
            return Err(AppError::new("terminal", format!("at most {MAX_USER_TERMINALS} terminals")));
        }
        let cwd = self
            .ctx()
            .map(|c| c.ws.root().to_path_buf())
            .ok()
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let (tx, mut rx) = mpsc::channel::<PtyEvent>(PTY_CHANNEL_CAPACITY);
        let pty = Arc::new(UserPty::spawn("powershell.exe", &cwd, rows, cols, tx)?);
        let id = self.next_term.fetch_add(1, Ordering::SeqCst);
        self.terms.lock().unwrap().insert(id, pty.clone());
        let terms = self.terms.clone();
        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::new();
            let mut exit: Option<Option<i32>> = None;
            while exit.is_none() {
                match rx.recv().await {
                    None => break,
                    Some(PtyEvent::Exited(c)) => exit = Some(c),
                    Some(PtyEvent::Output(b)) => {
                        buf.extend_from_slice(&b);
                        // coalesce whatever is already queued, up to 64 KB
                        while buf.len() < BATCH_BYTES {
                            match rx.try_recv() {
                                Ok(PtyEvent::Output(b)) => buf.extend_from_slice(&b),
                                Ok(PtyEvent::Exited(c)) => {
                                    exit = Some(c);
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }
                if !buf.is_empty() && !out(std::mem::take(&mut buf)) {
                    pty.kill();
                    terms.lock().unwrap().remove(&id);
                    return;
                }
            }
            terms.lock().unwrap().remove(&id);
            let _ = ui_tx.send(UiEvent::UserTerminalExit { id, code: exit.flatten() });
        });
        Ok(id)
    }

    fn pty(&self, id: u64) -> Result<Arc<UserPty>, AppError> {
        self.terms
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| AppError::new("terminal", "unknown terminal"))
    }

    /// Blocking write (the pty write can block); call from `spawn_blocking`.
    pub fn terminal_write(&self, id: u64, data: &[u8]) -> Result<(), AppError> {
        Ok(self.pty(id)?.write(data)?)
    }

    pub fn terminal_resize(&self, id: u64, rows: u16, cols: u16) -> Result<(), AppError> {
        Ok(self.pty(id)?.resize(rows, cols)?)
    }

    pub fn close_terminal(&self, id: u64) {
        if let Some(p) = self.terms.lock().unwrap().remove(&id) {
            p.kill();
        }
    }
}

/// Emit `Gpu` events every ~2 s while a window listener exists.
pub async fn run_gpu_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(GPU_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if !state.has_sink() {
            continue;
        }
        if let Ok(Some((used, total))) = tokio::task::spawn_blocking(probe::gpu_memory).await {
            state.emit(UiEvent::Gpu { used_mib: used, total_mib: total });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(s: &str) -> UiEvent { UiEvent::Chunk { text: s.into() } }
    fn text_of(batches: &[Vec<UiEvent>]) -> String {
        batches.iter().flatten().filter_map(|e| match e { UiEvent::Chunk { text } => Some(text.as_str()), _ => None }).collect()
    }

    #[tokio::test]
    async fn commands_reject_paths_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "s").unwrap();
        std::fs::write(ws.join("ok.txt"), "ok").unwrap();
        let st = AppState::new(dir.path().join("cfg"), None);
        // no workspace
        assert_eq!(st.list_dir("").unwrap_err().code, "no_workspace");
        assert_eq!(st.read_file("a").unwrap_err().code, "no_workspace");
        let dummy = StampDto { mtime_ms: 0, hash: "0".into() };
        assert_eq!(st.save_file("a", "x", &dummy).unwrap_err().code, "no_workspace");
        st.open_folder(ws.to_str().unwrap()).await.unwrap();
        let outside = dir.path().join("secret.txt").to_string_lossy().to_string();
        for bad in ["../x", r"..\x","../secret.txt", "C:x", "D:secret.txt", "/secret", outside.as_str()] {
            assert_eq!(st.list_dir(bad).unwrap_err().code, "escapes", "list {bad}");
            assert_eq!(st.read_file(bad).unwrap_err().code, "escapes", "read {bad}");
            assert_eq!(st.save_file(bad, "x", &dummy).unwrap_err().code, "escapes", "save {bad}");
        }
        assert!(std::fs::read_to_string(dir.path().join("secret.txt")).unwrap() == "s");
        assert_eq!(st.read_file("missing.txt").unwrap_err().code, "not_found");
    }

    #[tokio::test]
    async fn save_with_stale_stamp_returns_conflict() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one").unwrap();
        let st = AppState::new(dir.path().join("cfg"), None);
        st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
        let r = st.read_file("a.txt").unwrap();
        std::fs::write(dir.path().join("a.txt"), "changed elsewhere").unwrap();
        let e = st.save_file("a.txt", "mine", &r.stamp).unwrap_err();
        assert_eq!(e.code, "conflict");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "changed elsewhere");
    }

    #[test]
    fn coalescer_batches_and_caps() {
        let mut c = Coalescer::new(10);
        for _ in 0..5 { c.push(chunk("abcd")); }
        let b = c.take();
        assert_eq!(text_of(&b), "abcd".repeat(5));
        assert_eq!(b.len(), 2, "20 bytes at cap 10 -> 2 batches");
        for batch in &b {
            let n: usize = batch.iter().map(|e| match e { UiEvent::Chunk { text } => text.len(), _ => 0 }).sum();
            assert!(n <= 10);
            assert_eq!(batch.len(), 1, "adjacent chunks merge");
        }
    }

    #[test]
    fn coalescer_preserves_order_across_non_chunk_events() {
        let mut c = Coalescer::new(1024);
        c.push(chunk("a")); c.push(chunk("b"));
        c.push(UiEvent::TurnEnd { stop_reason: "end_turn".into() });
        c.push(chunk("c"));
        let b = c.take();
        let all: Vec<_> = b.into_iter().flatten().collect();
        assert_eq!(all, vec![chunk("ab"), UiEvent::TurnEnd { stop_reason: "end_turn".into() }, chunk("c")]);
    }

    #[test]
    fn coalescer_splits_oversized_chunk_without_loss() {
        let mut c = Coalescer::new(8);
        let big = "é".repeat(20); // 40 bytes, multi-byte
        c.push(chunk(&big));
        c.push(UiEvent::TurnEnd { stop_reason: "x".into() });
        let b = c.take();
        assert_eq!(text_of(&b), big);
        for batch in &b {
            for e in batch { if let UiEvent::Chunk { text } = e { assert!(text.len() <= 8); } }
        }
        assert!(matches!(b.last().unwrap().last().unwrap(), UiEvent::TurnEnd { .. }));
    }

    #[test]
    fn capabilities_have_no_shell_permission() {
        let caps_raw = include_str!("../capabilities/default.json");
        let caps: Value = serde_json::from_str(caps_raw).unwrap();
        let perms = caps["permissions"].as_array().expect("permissions array");
        assert!(!perms.is_empty());
        for p in perms {
            let id = p.as_str().or_else(|| p["identifier"].as_str()).expect("permission id");
            let l = id.to_ascii_lowercase();
            assert!(!l.contains("shell"), "shell permission: {id}");
            assert!(!l.starts_with("fs:") && !l.starts_with("fs-") && !l.contains("plugin-fs"), "fs permission: {id}");
            assert!(!l.contains("execute") && !l.contains("spawn"), "spawn permission: {id}");
            assert!(l.starts_with("core:"), "unexpected non-core permission: {id}");
        }
        // main window only
        assert_eq!(caps["windows"], json!(["main"]));

        let conf_raw = include_str!("../tauri.conf.json");
        let conf: Value = serde_json::from_str(conf_raw).unwrap();
        let l = conf_raw.to_ascii_lowercase();
        assert!(!l.contains("\"shell\"") && !l.contains("plugin-shell") && !l.contains("\"fs\""));
        if let Some(p) = conf.get("plugins").and_then(Value::as_object) {
            assert!(!p.contains_key("shell") && !p.contains_key("fs"));
        }
        let csp = conf["app"]["security"]["csp"].as_str().expect("csp present");
        assert_eq!(
            csp,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost; worker-src 'self' blob:; img-src 'self' data:; font-src 'self'"
        );
        assert!(!csp.contains("unsafe-eval"));
        let dev = conf["app"]["security"]["devCsp"].as_str().unwrap_or("");
        assert!(!dev.contains("unsafe-eval"));
        // no shell/fs plugin crates either
        let cargo = include_str!("../Cargo.toml");
        assert!(!cargo.contains("plugin-shell") && !cargo.contains("plugin-fs"));
    }

    #[test]
    fn map_update_maps_agent_payloads() {
        assert_eq!(
            map_update(&json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}})),
            Some(chunk("hi"))
        );
        assert_eq!(
            map_update(&json!({"sessionUpdate":"tool_call","toolCallId":"t1","title":"read_file a","kind":"read","status":"pending"})),
            Some(UiEvent::ToolCall { id: "t1".into(), title: "read_file a".into(), kind: "read".into(), status: "pending".into(), content: None })
        );
        assert_eq!(
            map_update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":"ok"})),
            Some(UiEvent::ToolUpdate { id: "t1".into(), status: "completed".into(), content: Some("ok".into()) })
        );
        assert_eq!(
            map_update(&json!({"sessionUpdate":"usage","promptTokens":10,"completionTokens":5,"tokensPerSecond":42.5})),
            Some(UiEvent::Usage { prompt_tokens: 10, completion_tokens: 5, tokens_per_second: 42.5 })
        );
        assert_eq!(map_update(&json!({"sessionUpdate":"plan"})), None);
        assert_eq!(map_update(&json!({"i": 1})), None);
    }

    #[test]
    fn ui_events_serialize_with_type_tag_and_camel_case() {
        let v = serde_json::to_value(UiEvent::TurnEnd { stop_reason: "end_turn".into() }).unwrap();
        assert_eq!(v, json!({"type":"turnEnd","stopReason":"end_turn"}));
        let v = serde_json::to_value(UiEvent::Gpu { used_mib: 1, total_mib: 8 }).unwrap();
        assert_eq!(v, json!({"type":"gpu","usedMiB":1,"totalMiB":8}));
        let v = serde_json::to_value(UiEvent::Approval {
            id: 3, kind: "command".into(), title: "run".into(), old: None, new: None, command: Some("cargo test".into()),
        }).unwrap();
        assert_eq!(v, json!({"type":"approval","id":3,"kind":"command","title":"run","command":"cargo test"}));
        let v = serde_json::to_value(UiEvent::ApplyEdit { id: 1, rel: "a.rs".into(), new_text: "x".into() }).unwrap();
        assert_eq!(v, json!({"type":"applyEdit","id":1,"rel":"a.rs","newText":"x"}));
        let v = serde_json::to_value(AppError::new("conflict", "m")).unwrap();
        assert_eq!(v, json!({"code":"conflict","message":"m"}));
    }

    #[tokio::test]
    async fn list_dir_sorts_dirs_first_and_gives_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/inner")).unwrap();
        std::fs::write(dir.path().join("b.txt"), "bb").unwrap();
        std::fs::write(dir.path().join("A.txt"), "a").unwrap();
        std::fs::write(dir.path().join("src/m.rs"), "").unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let st = AppState::new(cfg.path().to_path_buf(), None);
        st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
        let root: Vec<_> = st.list_dir("").unwrap().into_iter().map(|e| (e.rel, e.is_dir)).collect();
        assert_eq!(root, vec![("src".to_string(), true), ("A.txt".to_string(), false), ("b.txt".to_string(), false)]);
        let sub: Vec<_> = st.list_dir("src").unwrap().into_iter().map(|e| e.rel).collect();
        assert_eq!(sub, vec!["src/inner", "src/m.rs"]);
        assert_eq!(st.list_dir("nope").unwrap_err().code, "not_found");
    }

    #[tokio::test]
    async fn save_roundtrip_and_bad_stamp() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\r\ntwo\r\n").unwrap();
        let st = AppState::new(dir.path().join("cfg"), None);
        st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
        let r = st.read_file("a.txt").unwrap();
        assert_eq!(r.text, "one\r\ntwo\r\n");
        let s2 = st.save_file("a.txt", "one\r\nTWO\r\n", &r.stamp).unwrap();
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"one\r\nTWO\r\n");
        assert_ne!(s2.hash, r.stamp.hash);
        // the returned stamp is accepted for the next save
        st.save_file("a.txt", "three", &s2).unwrap();
        let bad = StampDto { mtime_ms: 0, hash: "zz".into() };
        assert_eq!(st.save_file("a.txt", "x", &bad).unwrap_err().code, "bad_request");
    }

    #[test]
    fn locate_agent_exe_prefers_override_then_sibling_then_target() {
        let d = tempfile::tempdir().unwrap();
        let name = format!("agent-core{}", std::env::consts::EXE_SUFFIX);
        let app_dir = d.path().join("app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let app = app_dir.join("shell.exe");
        assert_eq!(locate_agent_exe(&app, None), None);
        let tdir = d.path().join("target/debug");
        std::fs::create_dir_all(&tdir).unwrap();
        std::fs::write(tdir.join(&name), "").unwrap();
        assert_eq!(locate_agent_exe(&app, None), Some(tdir.join(&name)));
        std::fs::write(app_dir.join(&name), "").unwrap();
        assert_eq!(locate_agent_exe(&app, None), Some(app_dir.join(&name)));
        let ov = d.path().join("custom.exe");
        std::fs::write(&ov, "").unwrap();
        assert_eq!(locate_agent_exe(&app, ov.to_str()), Some(ov.clone()));
        assert_eq!(locate_agent_exe(&app, Some("Z:/does/not/exist.exe")), Some(app_dir.join(&name)));
    }

    #[test]
    fn settings_are_validated() {
        let cur = Settings::default();
        let mut s = cur.clone();
        s.endpoint = "ftp://x".into();
        assert_eq!(sanitize_settings(s, &cur).unwrap_err().code, "bad_request");
        let mut s = cur.clone();
        s.endpoint = " http://127.0.0.1:1234/ ".into();
        s.font_size = 99;
        s.ignore = vec![" dist ".into(), "".into(), "dist".into()];
        s.recent = vec!["C:/evil".into()];
        let ok = sanitize_settings(s, &cur).unwrap();
        assert_eq!(ok.endpoint, "http://127.0.0.1:1234");
        assert_eq!(ok.font_size, 32);
        assert_eq!(ok.ignore, vec!["dist"]);
        assert!(ok.recent.is_empty(), "recent is host-owned");
    }

    #[tokio::test]
    async fn agent_commands_report_missing_agent_and_bad_mode() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path().join("cfg"), None);
        assert_eq!(st.agent_prompt("plan", "x").await.unwrap_err().code, "no_workspace");
        st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
        assert_eq!(st.agent_prompt("yolo", "x").await.unwrap_err().code, "bad_request");
        assert_eq!(st.agent_prompt("plan", "x").await.unwrap_err().code, "agent_missing");
        assert_eq!(st.resolve_approval(1, true).await.unwrap_err().code, "agent_missing");
        st.agent_cancel(); // no agent: must not panic
    }

    #[tokio::test]
    async fn pump_delivers_events_in_order_and_coalesces_chunks() {
        let (tx, rx) = mpsc::unbounded_channel();
        let slot: SinkSlot = Arc::new(Mutex::new(None));
        let got = Arc::new(Mutex::new(Vec::<UiEvent>::new()));
        let g2 = got.clone();
        *slot.lock().unwrap() = Some(Arc::new(move |e: &UiEvent| {
            g2.lock().unwrap().push(e.clone());
            true
        }));
        let pump = tokio::spawn(run_pump(rx, slot.clone()));
        for i in 0..100 {
            tx.send(chunk(&format!("{i},"))).unwrap();
        }
        tx.send(UiEvent::TurnEnd { stop_reason: "end_turn".into() }).unwrap();
        drop(tx);
        pump.await.unwrap();
        let evs = got.lock().unwrap().clone();
        assert!(evs.len() < 10, "chunks merged, got {} events", evs.len());
        assert!(matches!(evs.last(), Some(UiEvent::TurnEnd { .. })));
        let text: String = evs
            .iter()
            .filter_map(|e| if let UiEvent::Chunk { text } = e { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(text, (0..100).map(|i| format!("{i},")).collect::<String>());
    }

    #[tokio::test]
    async fn pump_forgets_a_dead_sink() {
        let (tx, rx) = mpsc::unbounded_channel();
        let dead: Sink = Arc::new(|_: &UiEvent| false);
        let slot: SinkSlot = Arc::new(Mutex::new(Some(dead)));
        let pump = tokio::spawn(run_pump(rx, slot.clone()));
        tx.send(chunk("x")).unwrap();
        drop(tx);
        pump.await.unwrap();
        assert!(slot.lock().unwrap().is_none());
    }

    /// Drives the real agent-core binary (no model server is running, so the turn ends
    /// with an error) through open_folder -> initialize/session/new -> prompt -> TurnEnd.
    /// Skipped when agent-core has not been built.
    #[tokio::test]
    async fn real_agent_turn_reports_running_and_turn_end() {
        let me = std::env::current_exe().unwrap();
        let Some(exe) = locate_agent_exe(&me, None) else {
            eprintln!("skipped: agent-core not built");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let st = Arc::new(AppState::new(dir.path().join("cfg"), Some(exe)));
        // unreachable model endpoint on a closed port
        let mut s = st.get_settings();
        s.endpoint = "http://127.0.0.1:9".into();
        st.set_settings(s).await.unwrap();
        let got = Arc::new(Mutex::new(Vec::<UiEvent>::new()));
        let g2 = got.clone();
        st.set_sink(Arc::new(move |e: &UiEvent| {
            g2.lock().unwrap().push(e.clone());
            true
        }));
        tokio::spawn(run_pump(st.take_event_rx().unwrap(), st.sink_slot()));
        st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
        let r = st.agent_prompt("plan", "hello").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let evs = got.lock().unwrap().clone();
        eprintln!("prompt result: {r:?}\nevents: {evs:?}");
        assert!(evs.contains(&UiEvent::AgentState { state: "running".into() }), "{evs:?}");
        let last_turn_end = evs.iter().rposition(|e| matches!(e, UiEvent::TurnEnd { .. }));
        assert!(last_turn_end.is_some(), "TurnEnd missing: {evs:?}");
        // nothing from that turn may follow its TurnEnd
        assert!(!evs[last_turn_end.unwrap()..].iter().any(|e| matches!(e, UiEvent::Chunk { .. })));
        // the busy flag was released: a second prompt is not refused as busy
        let r2 = st.agent_prompt("plan", "again").await;
        assert_ne!(r2.err().map(|e| e.code), Some("busy".to_string()));
        st.shutdown().await;
    }
}
