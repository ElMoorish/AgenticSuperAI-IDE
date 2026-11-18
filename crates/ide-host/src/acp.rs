//! ACP wire layer: line-delimited JSON-RPC 2.0 framing and message classification.

use serde_json::Value;
use std::fmt;

/// Events surfaced from the supervised agent process.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// `session/update` notification from the agent.
    Update { session_id: String, update: Value },
    /// A request from the agent to the editor; answer with `Supervisor::respond`.
    Request { id: u64, method: String, params: Value },
    /// The process exited.
    Exited { code: Option<i32> },
    /// The process was restarted (`attempt` counts restarts, from 1).
    Restarted { attempt: u32 },
    /// Too many crashes in the window; no further restarts.
    GaveUp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AcpError {
    /// The agent process exited (or was not running) before answering.
    Exited,
    /// The agent returned a JSON-RPC error object.
    Remote(Value),
    Io(String),
    /// The supervisor was shut down.
    Closed,
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpError::Exited => write!(f, "agent process exited"),
            AcpError::Remote(v) => write!(f, "agent error: {v}"),
            AcpError::Io(s) => write!(f, "io error: {s}"),
            AcpError::Closed => write!(f, "supervisor closed"),
        }
    }
}

impl std::error::Error for AcpError {}

/// A decoded line from the agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response { id: u64, result: Result<Value, Value> },
    Notification { method: String, params: Value },
    Request { id: u64, method: String, params: Value },
}

/// Classify one JSON line. Errors are human-readable and meant for logging.
pub fn parse_line(line: &str) -> Result<Incoming, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let id = obj.get("id").filter(|i| !i.is_null());
    let method = obj.get("method").and_then(|m| m.as_str());
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(m)) => {
            let id = id.as_u64().ok_or("request id is not an unsigned integer")?;
            Ok(Incoming::Request { id, method: m.to_string(), params })
        }
        (None, Some(m)) => Ok(Incoming::Notification { method: m.to_string(), params }),
        (Some(id), None) => {
            let id = id.as_u64().ok_or("response id is not an unsigned integer")?;
            if let Some(err) = obj.get("error") {
                Ok(Incoming::Response { id, result: Err(err.clone()) })
            } else if let Some(res) = obj.get("result") {
                Ok(Incoming::Response { id, result: Ok(res.clone()) })
            } else {
                Err("response has neither result nor error".into())
            }
        }
        (None, None) => Err("message has neither id nor method".into()),
    }
}

fn line(v: Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

pub fn encode_request(id: u64, method: &str, params: Value) -> String {
    line(serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
}

pub fn encode_notification(method: &str, params: Value) -> String {
    line(serde_json::json!({"jsonrpc":"2.0","method":method,"params":params}))
}

pub fn encode_response(id: u64, result: Result<Value, Value>) -> String {
    match result {
        Ok(r) => line(serde_json::json!({"jsonrpc":"2.0","id":id,"result":r})),
        Err(e) => line(serde_json::json!({"jsonrpc":"2.0","id":id,"error":e})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_messages() {
        assert_eq!(
            parse_line(r#"{"id":3,"result":{"a":1}}"#).unwrap(),
            Incoming::Response { id: 3, result: Ok(json!({"a":1})) }
        );
        assert_eq!(
            parse_line(r#"{"id":3,"error":{"code":-1}}"#).unwrap(),
            Incoming::Response { id: 3, result: Err(json!({"code":-1})) }
        );
        assert_eq!(
            parse_line(r#"{"method":"session/update","params":{"sessionId":"s"}}"#).unwrap(),
            Incoming::Notification { method: "session/update".into(), params: json!({"sessionId":"s"}) }
        );
        assert_eq!(
            parse_line(r#"{"id":1000,"method":"fs/read_text_file","params":{"path":"x"}}"#).unwrap(),
            Incoming::Request { id: 1000, method: "fs/read_text_file".into(), params: json!({"path":"x"}) }
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line("[1]").is_err());
        assert!(parse_line("{}").is_err());
    }

    #[test]
    fn encodes_single_lines() {
        let s = encode_request(7, "m", json!({"t":"a\nb"}));
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
        assert!(encode_response(1, Err(json!("x"))).contains("\"error\""));
        assert!(!encode_notification("session/cancel", json!({})).contains("\"id\""));
    }
}
