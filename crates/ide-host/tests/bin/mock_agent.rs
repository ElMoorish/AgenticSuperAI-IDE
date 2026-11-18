//! Scriptable ACP test double. Behaviour comes from env MOCK_MODE, a comma list of:
//!   echo            answer every request with {"id":N,"result":{"echo":<params>}}
//!   ask             before answering, send agent request id 1000 fs/read_text_file and wait for its response
//!   exit-after=N    exit(0) after N ms
//!   crash           exit(3) immediately
//!   longline        send one ~1 MB single-line notification at start
use std::io::{BufRead, Write};

fn field_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let i = line.find(key)? + key.len();
    Some(&line[i..])
}

fn main() {
    let mode = std::env::var("MOCK_MODE").unwrap_or_default();
    let has = |f: &str| mode.split(',').any(|m| m == f);
    if has("crash") {
        std::process::exit(3);
    }
    if let Some(ms) = mode.split(',').find_map(|m| m.strip_prefix("exit-after=")) {
        let ms: u64 = ms.parse().unwrap_or(0);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            std::process::exit(0);
        });
    }
    let stdout = std::io::stdout();
    if has("longline") {
        let body = "h\u{e9}llo\u{1F600} ".repeat(100_000); // ~1 MB, multi-byte UTF-8
        let mut o = stdout.lock();
        writeln!(
            o,
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"text":"{body}"}}}}}}"#
        )
        .unwrap();
        o.flush().unwrap();
    }
    eprintln!("mock_agent started: {mode}");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        // Only requests (have a method and an id) are answered.
        if !line.contains("\"method\"") || !line.contains("\"id\"") {
            continue;
        }
        let id: String = field_after(&line, "\"id\":")
            .map(|r| r.chars().take_while(|c| c.is_ascii_digit()).collect())
            .unwrap_or_default();
        if let Some(n) = mode.split(',').find_map(|m| m.strip_prefix("flood=")) {
            let n: usize = n.parse().unwrap_or(0);
            let mut o = stdout.lock();
            for i in 0..n {
                writeln!(o, r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"i":{i}}}}}}}"#).unwrap();
            }
            o.flush().unwrap();
        }
        if has("askstr") {
            let mut o = stdout.lock();
            writeln!(o, r#"{{"jsonrpc":"2.0","id":"abc","method":"fs/read_text_file","params":{{"path":"a.txt"}}}}"#).unwrap();
            o.flush().unwrap();
            drop(o);
            while let Some(Ok(l)) = lines.next() {
                if l.contains("\"id\":\"abc\"") && !l.contains("\"method\"") {
                    break;
                }
            }
            let mut o = stdout.lock();
            writeln!(o, r#"{{"jsonrpc":"2.0","id":{id},"result":{{"asked":true}}}}"#).unwrap();
            o.flush().unwrap();
        } else if has("ask") {
            let mut o = stdout.lock();
            writeln!(o, r#"{{"jsonrpc":"2.0","id":1000,"method":"fs/read_text_file","params":{{"path":"a.txt"}}}}"#).unwrap();
            o.flush().unwrap();
            drop(o);
            while let Some(Ok(l)) = lines.next() {
                if l.contains("\"id\":1000") && !l.contains("\"method\"") {
                    break;
                }
            }
            let mut o = stdout.lock();
            writeln!(o, r#"{{"jsonrpc":"2.0","id":{id},"result":{{"asked":true}}}}"#).unwrap();
            o.flush().unwrap();
        } else if has("echo") {
            let params = field_after(&line, "\"params\":").unwrap_or("null");
            let params = params.trim_end().strip_suffix('}').unwrap_or(params);
            let mut o = stdout.lock();
            writeln!(o, r#"{{"jsonrpc":"2.0","id":{id},"result":{{"echo":{params}}}}}"#).unwrap();
            o.flush().unwrap();
        }
    }
}
