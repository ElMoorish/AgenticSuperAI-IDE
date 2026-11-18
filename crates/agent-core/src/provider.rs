//! Data-plane client for an OpenAI-compatible /v1/chat/completions (llama-server, Ollama, LM Studio),
//! with SSE streaming and native tool calls. Lifecycle control (spawn, flags, eviction) is a separate adapter.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub struct Provider {
    base: String,
    model: String,
}

#[derive(Debug, PartialEq)]
pub enum Delta {
    Text(String),
    /// One fragment of a native tool call; fragments with the same `index` belong together.
    Tool { index: usize, id: Option<String>, name: Option<String>, args: Option<String> },
}

#[derive(Debug, Default, PartialEq)]
pub struct ToolCallOut {
    pub id: String,
    pub name: String,
    /// Raw JSON text of the arguments (may be malformed; the caller repairs it).
    pub args: String,
}

#[derive(Debug, Default)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCallOut>,
    /// Set when we cut the stream ourselves (degenerate output or deadline). The text must not enter history.
    pub aborted: Option<String>,
}

/// True when the tail of the output is a tiny alphabet repeated (e.g. endless `\n\n\n`), i.e. the model is stuck.
pub fn is_degenerate(text: &str) -> bool {
    const WINDOW: usize = 300;
    let tail: Vec<char> = text.chars().rev().take(WINDOW).collect();
    if tail.len() < WINDOW {
        return false;
    }
    let mut distinct: Vec<char> = Vec::new();
    for c in tail {
        if !distinct.contains(&c) {
            distinct.push(c);
            if distinct.len() > 3 {
                return false;
            }
        }
    }
    true
}

/// One SSE line -> deltas. `Err(())` = [DONE]. Unparseable or unrelated lines yield no deltas.
pub fn parse_sse_line(line: &str) -> Result<Vec<Delta>, ()> {
    let Some(data) = line.strip_prefix("data:") else { return Ok(vec![]) };
    let data = data.trim();
    if data == "[DONE]" {
        return Err(());
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(vec![]) };
    let delta = &v["choices"][0]["delta"];
    let mut out = Vec::new();
    if let Some(t) = delta["content"].as_str().filter(|s| !s.is_empty()) {
        out.push(Delta::Text(t.to_string()));
    }
    if let Some(calls) = delta["tool_calls"].as_array() {
        for (i, c) in calls.iter().enumerate() {
            let f = &c["function"];
            // arguments is a string in OpenAI streams but some servers send an object
            let args = match &f["arguments"] {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                other => Some(other.to_string()),
            };
            out.push(Delta::Tool {
                index: c["index"].as_u64().map(|n| n as usize).unwrap_or(i),
                id: c["id"].as_str().map(String::from),
                name: f["name"].as_str().map(String::from),
                args,
            });
        }
    }
    Ok(out)
}

impl Provider {
    pub fn from_env() -> Self {
        Self {
            base: std::env::var("ASA_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            model: std::env::var("ASA_LLM_MODEL").unwrap_or_else(|_| "local".into()),
        }
    }

    /// Streams the completion, calling `on_text` per text chunk. Pass `tools` for native tool calling.
    pub fn chat_stream(&self, messages: &[Value], tools: Option<&[Value]>, mut on_text: impl FnMut(&str)) -> Result<Completion> {
        // max_tokens bounds a runaway generation server-side; the read timeout catches a stalled server;
        // the deadline and degenerate-output check below catch a stream that keeps talking but goes nowhere.
        let max_tokens = env_num("ASA_MAX_TOKENS", 4096);
        let mut body = json!({"model": self.model, "messages": messages, "temperature": 0.2, "stream": true, "max_tokens": max_tokens});
        if let Some(t) = tools {
            body["tools"] = json!(t);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(env_num("ASA_READ_TIMEOUT_S", 120)))
            .build();
        let deadline = Instant::now() + Duration::from_secs(env_num("ASA_DEADLINE_S", 300));
        let resp = agent
            .post(&format!("{}/v1/chat/completions", self.base))
            .send_json(body)
            .map_err(anyhow::Error::from)
            .context("llama-server not reachable or rejected the request (see config/start-llama-server.ps1)")?;
        let mut done = Completion::default();
        let mut calls: BTreeMap<usize, ToolCallOut> = BTreeMap::new();
        'stream: for line in BufReader::new(resp.into_reader()).lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    done.aborted = Some(format!("stream error: {e}"));
                    break;
                }
            };
            let Ok(deltas) = parse_sse_line(&line) else { break };
            for d in deltas {
                match d {
                    Delta::Text(t) => {
                        on_text(&t);
                        done.text.push_str(&t);
                        if is_degenerate(&done.text) {
                            done.aborted = Some("the model got stuck repeating itself".into());
                            break 'stream;
                        }
                        if Instant::now() > deadline {
                            done.aborted = Some("the model took too long to answer".into());
                            break 'stream;
                        }
                    }
                    Delta::Tool { index, id, name, args } => {
                        let c = calls.entry(index).or_default();
                        if let Some(id) = id.filter(|_| c.id.is_empty()) {
                            c.id = id;
                        }
                        if let Some(n) = name.filter(|_| c.name.is_empty()) {
                            c.name = n;
                        }
                        if let Some(a) = args {
                            c.args.push_str(&a);
                        }
                    }
                }
            }
        }
        done.tool_calls = calls.into_values().filter(|c| !c.name.is_empty()).collect();
        for (i, c) in done.tool_calls.iter_mut().enumerate() {
            if c.id.is_empty() {
                c.id = format!("call_{i}");
            }
        }
        Ok(done)
    }
}

/// True if the server rejected the request itself (e.g. the model does not support `tools`).
pub fn is_bad_request(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<ureq::Error>(), Some(ureq::Error::Status(400 | 422, _)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_detects_loops_not_code() {
        assert!(is_degenerate(&format!("ok {}", "\\n".repeat(200))));
        assert!(!is_degenerate("short"));
        let code = "fn main() { let total = compute(1, 2); println!(\"{}\", total); }\n".repeat(10);
        assert!(!is_degenerate(&code));
    }

    #[test]
    fn parses_text_delta() {
        let l = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(parse_sse_line(l), Ok(vec![Delta::Text("hi".into())]));
    }

    #[test]
    fn done_and_noise() {
        assert_eq!(parse_sse_line("data: [DONE]"), Err(()));
        assert_eq!(parse_sse_line(": keepalive"), Ok(vec![]));
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{}}]}"#), Ok(vec![]));
    }

    #[test]
    fn parses_tool_call_fragments() {
        let first = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#;
        let next = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]}}]}"#;
        assert_eq!(
            parse_sse_line(first),
            Ok(vec![Delta::Tool { index: 0, id: Some("c1".into()), name: Some("read_file".into()), args: Some("{\"pa".into()) }])
        );
        assert_eq!(parse_sse_line(next), Ok(vec![Delta::Tool { index: 0, id: None, name: None, args: Some("th\":\"a\"}".into()) }]));
    }

    #[test]
    fn object_arguments_are_serialized() {
        let l = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"x","arguments":{"a":1}}}]}}]}"#;
        match parse_sse_line(l).unwrap().remove(0) {
            Delta::Tool { args: Some(a), .. } => assert_eq!(a, r#"{"a":1}"#),
            d => panic!("{d:?}"),
        }
    }
}
