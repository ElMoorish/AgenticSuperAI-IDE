//! Tool-call repair for small local models (Doc A section D.1, Doc B constrained decoding).
//! Deterministic fixes first; a model retry is the caller's job if this returns Err.

use serde_json::Value;

/// Try to turn raw model output into a JSON tool call.
pub fn repair_tool_call(raw: &str) -> Result<Value, String> {
    let body = extract_tool_call_block(raw).unwrap_or(raw).trim();
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return Ok(v);
    }
    let stripped = strip_trailing_empty_object(body);
    if let Ok(v) = serde_json::from_str::<Value>(stripped) {
        return Ok(v);
    }
    // Structural fix first, then escapes: \n, \t etc. are valid JSON and must survive.
    for candidate in [stripped.to_string(), insert_missing_arguments_key(stripped)] {
        let escaped = escape_bad_backslashes(&candidate);
        if let Ok(v) = serde_json::from_str::<Value>(&escaped) {
            return Ok(v);
        }
    }
    serde_json::from_str::<Value>(stripped).map_err(|e| format!("unrepairable tool call: {e}"))
}

/// Small models often write {"name":"tool": {...}} and forget the "arguments" key.
fn insert_missing_arguments_key(s: &str) -> String {
    let Some(i) = s.find("\"name\"") else { return s.to_string() };
    let after = &s[i + 6..];
    let Some(colon) = after.find(':') else { return s.to_string() };
    let rest = &after[colon + 1..];
    let trimmed = rest.trim_start();
    if !trimmed.starts_with('"') {
        return s.to_string();
    }
    let Some(end) = trimmed[1..].find('"') else { return s.to_string() };
    let after_name = &trimmed[end + 2..];
    let ws = after_name.len() - after_name.trim_start().len();
    if !after_name[ws..].starts_with(':') {
        return s.to_string();
    }
    let consumed = s.len() - after_name.len() + ws; // index of the stray ':'
    format!("{},\"arguments\":{}", &s[..consumed], &s[consumed + 1..])
}

/// Pull the payload out of `<tool_call>...</tool_call>` (Qwen-style XML fallback).
fn extract_tool_call_block(s: &str) -> Option<&str> {
    let start = s.find("<tool_call>")? + "<tool_call>".len();
    let end = s[start..].find("</tool_call>").map(|i| start + i).unwrap_or(s.len());
    Some(&s[start..end])
}

/// OpenCode issue #64404: models append a stray `{}` after the JSON object.
fn strip_trailing_empty_object(s: &str) -> &str {
    let t = s.trim_end();
    t.strip_suffix("{}").map(str::trim_end).unwrap_or(t)
}

/// Windows paths like C:\src\file.rs break JSON; double any backslash that is not a valid escape.
fn escape_bad_backslashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            match chars.get(i + 1) {
                Some('"') | Some('\\') | Some('/') | Some('u') | Some('n') | Some('t') | Some('r') | Some('b') | Some('f') => {
                    out.push('\\');
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                _ => out.push_str("\\\\"),
            }
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

/// Small models often print a tool call as bare JSON text instead of using the tool channel, e.g.
/// `{"name":"read_file","parameters":{"path":"a"}}` or `{"type":"function","function":{"name":..,"arguments":..}}`.
/// Returns (name, arguments) for the first object naming one of `allowed`.
pub fn extract_text_tool_call(text: &str, allowed: &[&str]) -> Option<(String, Value)> {
    for (i, _) in text.match_indices('{') {
        let Some(Ok(v)) = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>().next() else { continue };
        let f = if v["function"].is_object() { &v["function"] } else { &v };
        let Some(name) = f["name"].as_str().filter(|n| allowed.contains(n)) else { continue };
        let args = [&f["arguments"], &f["parameters"]].into_iter().find(|a| !a.is_null()).cloned().unwrap_or(Value::Object(Default::default()));
        // some models double-encode arguments as a JSON string
        let args = match &args {
            Value::String(s) => serde_json::from_str(s).unwrap_or(args),
            _ => args,
        };
        return Some((name.to_string(), args));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_tool_calls_both_shapes() {
        let ok = ["read_file", "run_compiler_check"];
        let a = extract_text_tool_call(r#"sure {"name":"read_file","parameters":{"path":"a.rs"}}"#, &ok).unwrap();
        assert_eq!(a.0, "read_file");
        assert_eq!(a.1["path"], "a.rs");
        let b = extract_text_tool_call(r#"{"name":"multiply","parameters":{}}
{"type":"function","function":{"name":"run_compiler_check","parameters":{"command":"cargo check"}}}"#, &ok).unwrap();
        assert_eq!(b.0, "run_compiler_check");
        assert_eq!(b.1["command"], "cargo check");
        assert!(extract_text_tool_call("no json here", &ok).is_none());
    }

    #[test]
    fn valid_json_passes_through() {
        let v = repair_tool_call(r#"{"name":"read_file","arguments":{"path":"a.rs"}}"#).unwrap();
        assert_eq!(v["name"], "read_file");
    }

    #[test]
    fn strips_trailing_empty_object() {
        let v = repair_tool_call(r#"{"name":"run","arguments":{}}{}"#).unwrap();
        assert_eq!(v["name"], "run");
    }

    #[test]
    fn extracts_xml_block() {
        let raw = "thinking...\n<tool_call>\n{\"name\":\"search\",\"arguments\":{}}\n</tool_call>";
        assert_eq!(repair_tool_call(raw).unwrap()["name"], "search");
    }

    #[test]
    fn escapes_windows_paths() {
        let v = repair_tool_call(r#"{"name":"read_file","arguments":{"path":"C:\src\main.rs"}}"#).unwrap();
        assert_eq!(v["arguments"]["path"], "C:\\src\\main.rs");
    }

    #[test]
    fn inserts_missing_arguments_key() {
        let v = repair_tool_call(r#"<tool_call>{"name":"run_compiler_check": {}}</tool_call>"#).unwrap();
        assert_eq!(v["name"], "run_compiler_check");
        assert!(v["arguments"].is_object());
        let v = repair_tool_call(r#"{"name":"read_file": {"path": "src/main.rs"}}"#).unwrap();
        assert_eq!(v["arguments"]["path"], "src/main.rs");
    }

    #[test]
    fn newlines_survive_repair() {
        // seen from a real 4B model: missing "arguments" key AND a diff containing \n escapes
        let raw = r#"<tool_call>{"name":"edit": {"path":"a.rs","diff":"<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE"}}</tool_call>"#;
        let v = repair_tool_call(raw).unwrap();
        assert!(v["arguments"]["diff"].as_str().unwrap().contains("SEARCH\nold\n"));
    }

    #[test]
    fn garbage_errors() {
        assert!(repair_tool_call("not json at all").is_err());
    }
}
