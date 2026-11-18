//! Structured `session/update` payloads (the `update` object) for tool calls and usage.
//! Wire format is consumed by the IDE shell; keep field names stable.

use serde_json::{json, Value};

const MAX_CONTENT: usize = 4000;

/// ACP tool category for one of our tools.
pub fn kind_for(tool: &str) -> &'static str {
    match tool {
        "read_file" => "read",
        "edit_file_search_replace" => "edit",
        "run_shell_scoped" | "run_compiler_check" => "execute",
        _ => "other",
    }
}

/// `tool_call` with an explicit kind.
pub fn tool_call_kind(id: &str, title: &str, kind: &str, status: &str) -> Value {
    json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": kind, "status": status})
}

/// `tool_call`; the kind is derived from the first word of the title (the tool name).
pub fn tool_call(id: &str, title: &str, status: &str) -> Value {
    tool_call_kind(
        id,
        title,
        kind_for(title.split_whitespace().next().unwrap_or("")),
        status,
    )
}

/// `tool_call_update`; content is truncated to 4,000 chars plus a notice.
pub fn tool_update(id: &str, status: &str, content: &str) -> Value {
    let total = content.chars().count();
    let content = if total > MAX_CONTENT {
        let head: String = content.chars().take(MAX_CONTENT).collect();
        format!("{head}\n... [truncated {} chars]", total - MAX_CONTENT)
    } else {
        content.to_string()
    };
    json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": status, "content": content})
}

pub fn usage(p: u64, c: u64, tps: f64) -> Value {
    json!({"sessionUpdate": "usage", "promptTokens": p, "completionTokens": c, "tokensPerSecond": tps})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_update_truncates_content() {
        let v = tool_update("t1", "completed", &"x".repeat(5000));
        let c = v["content"].as_str().unwrap();
        assert!(c.starts_with(&"x".repeat(4000)));
        assert!(
            c.chars().count() > 4000 && c.chars().count() <= 4100,
            "notice appended"
        );
        assert!(c.contains("truncated 1000"));
        assert_eq!(tool_update("t1", "completed", "short")["content"], "short");
    }

    #[test]
    fn usage_shape() {
        let v = usage(10, 20, 1.5);
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "completionTokens",
                "promptTokens",
                "sessionUpdate",
                "tokensPerSecond"
            ]
            .map(|k| k)
        );
        assert_eq!(v["sessionUpdate"], "usage");
    }

    #[test]
    fn tool_call_shape_and_kinds() {
        let v = tool_call("a", "read_file a.txt", "pending");
        assert_eq!(
            (
                v["kind"].as_str(),
                v["status"].as_str(),
                v["toolCallId"].as_str()
            ),
            (Some("read"), Some("pending"), Some("a"))
        );
        assert_eq!(kind_for("edit_file_search_replace"), "edit");
        assert_eq!(kind_for("run_compiler_check"), "execute");
        assert_eq!(kind_for("run_shell_scoped"), "execute");
    }
}
