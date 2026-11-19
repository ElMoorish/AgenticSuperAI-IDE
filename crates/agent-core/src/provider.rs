//! Data-plane client for an OpenAI-compatible /v1/chat/completions (llama-server, Ollama, LM Studio),
//! with SSE streaming and native tool calls. Lifecycle control (spawn, flags, eviction) is a separate adapter.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
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
    /// Server-reported token usage (`usage` object in the stream).
    Usage { prompt: u64, completion: u64 },
}

#[derive(Debug, Default, PartialEq)]
pub struct ToolCallOut {
    pub id: String,
    pub name: String,
    /// Raw JSON text of the arguments (may be malformed; the caller repairs it).
    pub args: String,
}

/// Worker-thread output of one request.
enum Ev {
    Failed(anyhow::Error),
    Line(Result<String, String>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
}

/// ceil(chars / 4): the token estimate used when the server reports no usage.
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

/// Estimated prompt tokens: total chars of the request messages / 4.
pub fn estimate_prompt_tokens(messages: &[Value]) -> u64 {
    let chars: usize = messages.iter().map(|m| m["content"].as_str().map(|s| s.chars().count()).unwrap_or(0)).sum();
    estimate_tokens(chars)
}

#[derive(Debug, Default)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCallOut>,
    /// Set when we cut the stream ourselves (degenerate output or deadline). The text must not enter history.
    pub aborted: Option<String>,
    /// Server-reported usage when present, else an estimate (chars/4).
    pub usage: Option<Usage>,
    pub tokens_per_second: f64,
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
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        if let (Some(p), Some(c)) = (u["prompt_tokens"].as_u64(), u["completion_tokens"].as_u64()) {
            out.push(Delta::Usage { prompt: p, completion: c });
        }
    }
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

/// True if this SSE line carries the first piece of generated output of any kind (visible text, hidden
/// reasoning, or a tool call). Prompt processing ends there, so generation speed is measured from it.
pub fn carries_generation(line: &str) -> bool {
    let Some(data) = line.strip_prefix("data:") else { return false };
    let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { return false };
    let d = &v["choices"][0]["delta"];
    let text = |k: &str| d[k].as_str().is_some_and(|s| !s.is_empty());
    text("content") || text("reasoning_content") || text("reasoning") || d["tool_calls"].as_array().is_some_and(|a| !a.is_empty())
}

/// Characters of hidden reasoning in this SSE line. They are generated tokens too: without them a
/// long think followed by a short answer would make the estimated speed and usage far too small.
pub fn reasoning_chars(line: &str) -> usize {
    let Some(data) = line.strip_prefix("data:") else { return 0 };
    let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { return 0 };
    let d = &v["choices"][0]["delta"];
    ["reasoning_content", "reasoning"].iter().filter_map(|k| d[*k].as_str()).map(|s| s.chars().count()).sum()
}

/// Tokens per second of generation. `window` is the time from the first generated piece to the end;
/// when it is too short to measure (a one-shot reply), fall back to the whole request time.
pub fn generation_speed(completion: u64, window: Option<f64>, total: f64) -> f64 {
    match window {
        Some(w) if w >= 0.2 && completion > 1 => (completion - 1) as f64 / w,
        _ if total > 0.0 => completion as f64 / total,
        _ => 0.0,
    }
}

/// The endpoint may be given with or without a trailing slash or `/v1` (OpenAI-style base URLs
/// end in `/v1`); the request path appended later already contains `/v1`.
pub fn normalize_base(raw: &str) -> String {
    let b = raw.trim().trim_end_matches('/');
    b.strip_suffix("/v1").unwrap_or(b).trim_end_matches('/').to_string()
}

impl Provider {
    pub fn from_env() -> Self {
        Self {
            base: normalize_base(&std::env::var("ASA_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into())),
            model: std::env::var("ASA_LLM_MODEL").unwrap_or_else(|_| "local".into()),
        }
    }

    /// Streams the completion, calling `on_text` per text chunk. Pass `tools` for native tool calling.
    /// When `cancel` becomes true the stream is dropped and `aborted` is `Some("cancelled")`.
    pub fn chat_stream(&self, messages: &[Value], tools: Option<&[Value]>, cancel: &AtomicBool, mut on_text: impl FnMut(&str)) -> Result<Completion> {
        let started = Instant::now();
        if cancel.load(Ordering::SeqCst) {
            return Ok(Completion { aborted: Some("cancelled".into()), ..Default::default() });
        }
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
        // The request and the body reads run on a worker thread so a cancel is honored even while the
        // server is still prefilling (no headers yet); this thread polls the channel and the cancel flag.
        let (tx, rx) = mpsc::channel::<Ev>();
        let url = format!("{}/v1/chat/completions", self.base);
        std::thread::spawn(move || {
            let mut req = agent.post(&url);
            if let Ok(k) = std::env::var("ASA_LLM_KEY") {
                if !k.trim().is_empty() {
                    req = req.set("Authorization", &format!("Bearer {}", k.trim()));
                }
            }
            let resp = match req.send_json(body) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Ev::Failed(anyhow::Error::from(e).context("llama-server not reachable or rejected the request (see config/start-llama-server.ps1)")));
                    return;
                }
            };
            for line in BufReader::new(resp.into_reader()).lines() {
                if tx.send(Ev::Line(line.map_err(|e| e.to_string()))).is_err() {
                    return; // receiver gone (cancelled or finished): drop the connection
                }
            }
        });
        let mut done = Completion::default();
        let mut calls: BTreeMap<usize, ToolCallOut> = BTreeMap::new();
        let mut streamed_chars = 0usize;
        let mut reported: Option<Usage> = None;
        let mut first_gen: Option<Instant> = None;
        'stream: loop {
            if cancel.load(Ordering::SeqCst) {
                done.aborted = Some("cancelled".into());
                break;
            }
            let line = match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ev::Failed(e)) => return Err(e),
                Ok(Ev::Line(Ok(l))) => l,
                Ok(Ev::Line(Err(e))) => {
                    done.aborted = Some(format!("stream error: {e}"));
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() > deadline {
                        done.aborted = Some("the model took too long to answer".into());
                        break;
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if first_gen.is_none() && carries_generation(&line) {
                first_gen = Some(Instant::now());
            }
            let Ok(deltas) = parse_sse_line(&line) else { break };
            streamed_chars += reasoning_chars(&line);
            for d in deltas {
                match d {
                    Delta::Usage { prompt, completion } => reported = Some(Usage { prompt, completion }),
                    Delta::Text(t) => {
                        streamed_chars += t.chars().count();
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
                            streamed_chars += a.chars().count();
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
        let usage = reported.unwrap_or(Usage { prompt: estimate_prompt_tokens(messages), completion: estimate_tokens(streamed_chars) });
        done.tokens_per_second = generation_speed(usage.completion, first_gen.map(|t| t.elapsed().as_secs_f64()), started.elapsed().as_secs_f64());
        done.usage = Some(usage);
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
    fn hidden_reasoning_counts_as_generated_text() {
        assert_eq!(reasoning_chars(r#"data: {"choices":[{"delta":{"reasoning_content":"abcd"}}]}"#), 4);
        assert_eq!(reasoning_chars(r#"data: {"choices":[{"delta":{"reasoning":"xy","content":"zzz"}}]}"#), 2);
        assert_eq!(reasoning_chars(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#), 0);
        assert_eq!(reasoning_chars("data: [DONE]"), 0);
    }

    #[test]
    fn parses_usage_chunk() {
        let l = r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#;
        assert_eq!(parse_sse_line(l), Ok(vec![Delta::Usage { prompt: 12, completion: 5 }]));
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{"content":"a"}}],"usage":null}"#), Ok(vec![Delta::Text("a".into())]));
    }

    #[test]
    fn token_estimates_round_up() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(8), 2);
        assert_eq!(estimate_tokens(9), 3);
        let msgs = vec![json!({"role":"user","content":"abcdefgh"}), json!({"role":"assistant","content":null})];
        assert_eq!(estimate_prompt_tokens(&msgs), 2);
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

#[cfg(test)]
mod speed_tests {
    use super::{carries_generation, generation_speed};

    #[test]
    fn generation_starts_at_the_first_piece_of_output_of_any_kind() {
        assert!(carries_generation(r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#));
        assert!(carries_generation(r#"data: {"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#));
        assert!(carries_generation(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0}]}}]}"#));
        // the opening role-only chunk, keep-alives and [DONE] are not generation
        assert!(!carries_generation(r#"data: {"choices":[{"delta":{"role":"assistant","content":""}}]}"#));
        assert!(!carries_generation(": keep-alive"));
        assert!(!carries_generation("data: [DONE]"));
    }

    #[test]
    fn speed_excludes_prompt_processing() {
        // 100 tokens, 60 s of prompt processing, then 10 s of generation: ~9.9 tok/s, not 100/70
        let v = generation_speed(100, Some(10.0), 70.0);
        assert!((v - 9.9).abs() < 0.01, "{v}");
        // a reply too short to time falls back to the whole request time
        assert!((generation_speed(20, Some(0.05), 4.0) - 5.0).abs() < 1e-9);
        assert!((generation_speed(20, None, 4.0) - 5.0).abs() < 1e-9);
        assert_eq!(generation_speed(0, None, 0.0), 0.0);
    }
}

#[cfg(test)]
mod base_tests {
    use super::normalize_base;

    #[test]
    fn endpoint_spellings_all_give_one_v1() {
        for raw in ["http://localhost:8888", "http://localhost:8888/", "http://localhost:8888/v1", "http://localhost:8888/v1/", " http://localhost:8888/v1 "] {
            assert_eq!(normalize_base(raw), "http://localhost:8888", "{raw}");
        }
        assert_eq!(normalize_base("http://h/proxy/v1"), "http://h/proxy");
    }
}
