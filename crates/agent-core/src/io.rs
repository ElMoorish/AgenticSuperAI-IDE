//! Line-delimited JSON-RPC transport. Besides answering the editor, the agent can call the
//! editor (fs/read_text_file, fs/write_text_file, session/request_permission) and block on the reply.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub struct Io<R: BufRead, W: Write> {
    input: R,
    out: W,
    next_id: u64,
}

impl<R: BufRead, W: Write> Io<R, W> {
    pub fn new(input: R, out: W) -> Self {
        Self { input, out, next_id: 1000 }
    }

    pub fn send(&mut self, v: Value) {
        let _ = writeln!(self.out, "{v}");
        let _ = self.out.flush();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    /// Next non-empty line parsed as JSON; `None` at EOF.
    pub fn read_msg(&mut self) -> Option<Result<Value, String>> {
        loop {
            let mut line = String::new();
            if self.input.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if !line.trim().is_empty() {
                return Some(serde_json::from_str(&line).map_err(|e| e.to_string()));
            }
        }
    }

    /// Call the editor and wait for the matching response. Unrelated messages (notifications
    /// such as session/cancel) are ignored while waiting.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            let msg = self.read_msg().ok_or("editor closed the connection")??;
            if msg["id"] == json!(id) && msg.get("method").is_none() {
                return match msg.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("editor error").to_string()),
                    None => Ok(msg["result"].clone()),
                };
            }
        }
    }
}
