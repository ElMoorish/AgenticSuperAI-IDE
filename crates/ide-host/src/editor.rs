//! Handles agent-initiated client requests (`fs/*`, `session/request_permission`).
//! Writes and permissions never act directly: they yield pending values that
//! the UI must approve.

use crate::files::read_stamped;
use crate::workspace::{FsError, Workspace};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// UI-owned (possibly dirty) content of open editor buffers.
pub trait BufferView: Send + Sync {
    fn open_buffer(&self, abs: &Path) -> Option<String>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingEdit {
    /// Host-assigned UI approval id.
    pub id: u64,
    /// The agent's JSON-RPC request id, to answer later.
    pub agent_id: Value,
    /// Workspace-relative, forward-slash path.
    pub path: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingCommand {
    pub id: u64,
    pub agent_id: Value,
    pub title: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HandlerOutcome {
    /// JSON-RPC result to send.
    Reply(Value),
    /// JSON-RPC error object to send.
    Error(Value),
    NeedsApproval(PendingEdit),
    NeedsPermission(PendingCommand),
}

pub struct EditorHandler<B: BufferView> {
    ws: Workspace,
    buffers: B,
    next_id: AtomicU64,
}

fn err(code: i64, message: impl Into<String>) -> HandlerOutcome {
    HandlerOutcome::Error(json!({"code": code, "message": message.into()}))
}

fn fs_err(e: &FsError) -> HandlerOutcome {
    let code = if matches!(e, FsError::NotFound(_)) {
        -32002
    } else {
        -32000
    };
    err(code, e.to_string())
}

impl<B: BufferView> EditorHandler<B> {
    pub fn new(ws: Workspace, buffers: B) -> Self {
        EditorHandler {
            ws,
            buffers,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn buffers(&self) -> &B {
        &self.buffers
    }

    fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn handle(&self, id: Value, method: &str, params: &Value) -> HandlerOutcome {
        match method {
            "fs/read_text_file" => {
                let Some(path) = params.get("path").and_then(Value::as_str) else {
                    return err(-32602, "missing string param: path");
                };
                match self.current_text(path) {
                    Ok(Some(content)) => HandlerOutcome::Reply(json!({ "content": content })),
                    Ok(None) => fs_err(&FsError::NotFound(path.to_string())),
                    Err(e) => fs_err(&e),
                }
            }
            "fs/write_text_file" => {
                let (Some(path), Some(new)) = (
                    params.get("path").and_then(Value::as_str),
                    params.get("content").and_then(Value::as_str),
                ) else {
                    return err(-32602, "missing string params: path, content");
                };
                let abs = match self.ws.resolve(path) {
                    Ok(a) => a,
                    Err(e) => return fs_err(&e),
                };
                let old = match self.current_text(path) {
                    Ok(t) => t.unwrap_or_default(),
                    Err(e) => return fs_err(&e),
                };
                let rel = abs
                    .strip_prefix(self.ws.root())
                    .unwrap_or(&abs)
                    .to_string_lossy()
                    .replace('\\', "/");
                HandlerOutcome::NeedsApproval(PendingEdit {
                    id: self.next(),
                    agent_id: id,
                    path: rel,
                    old,
                    new: new.to_string(),
                })
            }
            "session/request_permission" => {
                let tc = params.get("toolCall").unwrap_or(&Value::Null);
                let title = tc
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let command = match tc.get("content") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(v) => v.to_string(),
                };
                HandlerOutcome::NeedsPermission(PendingCommand {
                    id: self.next(),
                    agent_id: id,
                    title,
                    command,
                })
            }
            _ => err(-32601, format!("method not found: {method}")),
        }
    }

    /// Open buffer first, else disk. `Ok(None)` = file does not exist.
    fn current_text(&self, path: &str) -> Result<Option<String>, FsError> {
        let abs = self.ws.resolve(path)?;
        if let Some(t) = self.buffers.open_buffer(&abs) {
            return Ok(Some(t));
        }
        match read_stamped(&self.ws, path) {
            Ok((t, _)) => Ok(Some(t)),
            Err(FsError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
