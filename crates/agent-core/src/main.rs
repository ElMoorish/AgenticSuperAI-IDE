#![allow(dead_code)] // budget module is tested but not wired into the loop yet
//! asa-agent: ACP-style JSON-RPC agent over line-delimited stdio.
//! The editor owns files, terminals and permissions; this process reasons, plans and asks the
//! editor to act. Implemented: initialize, session/new, session/prompt (streamed, tool loop).

mod agent;
mod budget;
mod diff;
mod events;
mod io;
mod loopguard;
mod provider;
mod repair;
mod session;
mod shell;

use agent::Mode;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;

fn main() {
    let prov = provider::Provider::from_env();
    let cancel = Arc::new(AtomicBool::new(false));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let rx = io::spawn_reader(std::io::BufReader::new(std::io::stdin()), cancel.clone(), in_flight.clone());
    let mut io = io::Io::new(rx, std::io::stdout().lock(), cancel, in_flight);
    let mut sessions: HashMap<String, session::Session> = HashMap::new();
    let mut counter = 0u32;

    while let Some(msg) = io.read_msg() {
        let req: Value = match msg {
            Ok(v) => v,
            Err(e) => {
                io.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e}}));
                continue;
            }
        };
        if req.get("method").is_none() {
            continue; // a late reply to a call abandoned by cancel
        }
        let id = req.get("id").cloned();
        let params = &req["params"];
        let res: Result<Value, (i32, String)> = match req["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "protocolVersion": 1,
                "agentInfo": {"name": "asa-agent", "version": env!("CARGO_PKG_VERSION")},
                "agentCapabilities": {"loadSession": false}
            })),
            "session/new" => {
                counter += 1;
                let sid = format!("s{counter}");
                let cwd = params["cwd"].as_str().unwrap_or("").to_string();
                let s = match params.get("history") {
                    Some(h) => session::Session::with_history(sid.clone(), cwd, h),
                    None => session::Session::new(sid.clone(), cwd),
                };
                let replayed = s.history_len();
                sessions.insert(sid.clone(), s);
                if replayed > 0 {
                    eprintln!("session {sid}: replayed {replayed} messages");
                }
                Ok(json!({"sessionId": sid}))
            }
            "session/prompt" => {
                let sid = params["sessionId"].as_str().unwrap_or("");
                let mode: Mode = serde_json::from_value(params["mode"].clone()).unwrap_or(Mode::Plan);
                let text = params["prompt"].as_array().and_then(|a| a.iter().find_map(|b| b["text"].as_str())).unwrap_or("");
                match sessions.get_mut(sid) {
                    Some(s) => {
                        let stop = s.prompt(&mut io, &prov, mode, text);
                        io.end_turn();
                        Ok(json!({"stopReason": stop}))
                    }
                    None => {
                        io.end_turn();
                        Err((-32602, format!("unknown session {sid}")))
                    }
                }
            }
            "session/cancel" => Ok(Value::Null),
            m => Err((-32601, format!("method not found: {m}"))),
        };
        if let Some(id) = id {
            io.send(match res {
                Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
                Err((c, m)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":c,"message":m}}),
            });
        }
    }
}
