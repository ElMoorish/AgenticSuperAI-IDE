//! Line-delimited JSON-RPC transport. Stdin is read on a dedicated thread (`spawn_reader`) that
//! feeds a channel, so `session/cancel` is noticed while a prompt is running. Besides answering the
//! editor, the agent can call the editor (fs/*, terminal/*, session/request_permission) and block on the reply.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

pub type Msg = Result<Value, String>;

/// Read lines from `input` on a background thread. Every `session/prompt` read bumps `in_flight` (the
/// main loop calls `Io::end_turn` when the turn finishes). A `session/cancel` sets `cancel` only if a
/// prompt is in flight, so a stale cancel cannot kill a later turn. The reader never clears the flag:
/// `end_turn` does, so cancel + immediate new prompt still cancels the active turn. The channel closes at EOF.
pub fn spawn_reader<R: BufRead + Send + 'static>(mut input: R, cancel: Arc<AtomicBool>, in_flight: Arc<AtomicUsize>) -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        let msg: Msg = serde_json::from_str(&line).map_err(|e| e.to_string());
        if let Ok(v) = &msg {
            match v["method"].as_str() {
                Some("session/cancel") => {
                    if in_flight.load(Ordering::SeqCst) > 0 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
                Some("session/prompt") => {
                    in_flight.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
        if tx.send(msg).is_err() {
            break;
        }
    });
    rx
}

pub struct Io<W: Write> {
    rx: Receiver<Msg>,
    out: W,
    next_id: u64,
    /// Messages that arrived during a `call` but were not its response; the main loop sees them first.
    pending: VecDeque<Msg>,
    cancel: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
}

impl<W: Write> Io<W> {
    pub fn new(rx: Receiver<Msg>, out: W, cancel: Arc<AtomicBool>, in_flight: Arc<AtomicUsize>) -> Self {
        Self { rx, out, next_id: 1000, pending: VecDeque::new(), cancel, in_flight }
    }

    pub fn send(&mut self, v: Value) {
        let _ = writeln!(self.out, "{v}");
        let _ = self.out.flush();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Call when a prompt turn finishes: it is no longer in flight and its cancel (if any) is consumed.
    pub fn end_turn(&self) {
        let _ = self.in_flight.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
        self.cancel.store(false, Ordering::SeqCst);
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Next message (queued ones first); `None` once stdin is closed and the queue is empty.
    pub fn read_msg(&mut self) -> Option<Msg> {
        self.pending.pop_front().or_else(|| self.rx.recv().ok())
    }

    /// Call the editor and wait for the matching response. Other messages are queued for the main
    /// loop, not dropped. A `session/cancel` (or EOF) fails the wait with an error.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        loop {
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(msg)) if msg["id"] == json!(id) && msg.get("method").is_none() => {
                    return match msg.get("error") {
                        Some(e) => Err(e["message"].as_str().unwrap_or("editor error").to_string()),
                        None => Ok(msg["result"].clone()),
                    };
                }
                Ok(other) => self.pending.push_back(other),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("editor closed the connection".into()),
            }
            if self.cancelled() {
                return Err("cancelled".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn io_with(msgs: &[Value]) -> (Io<Vec<u8>>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        for m in msgs {
            tx.send(Ok(m.clone())).unwrap();
        }
        std::mem::forget(tx); // keep the channel open so call() waits rather than seeing EOF
        let cancel = Arc::new(AtomicBool::new(false));
        (Io::new(rx, Vec::new(), cancel.clone(), Arc::new(AtomicUsize::new(0))), cancel)
    }

    #[test]
    fn call_queues_unrelated_messages() {
        let (mut io, _) = io_with(&[json!({"jsonrpc":"2.0","method":"session/prompt","id":7,"params":{}}), json!({"jsonrpc":"2.0","id":1000,"result":{"ok":true}})]);
        assert_eq!(io.call("fs/x", json!({})).unwrap(), json!({"ok":true}));
        let queued = io.read_msg().unwrap().unwrap();
        assert_eq!(queued["method"], "session/prompt");
    }

    #[test]
    fn call_reports_editor_error() {
        let (mut io, _) = io_with(&[json!({"id":1000,"error":{"message":"nope"}})]);
        assert_eq!(io.call("fs/x", json!({})), Err("nope".to_string()));
    }

    #[test]
    fn cancel_fails_pending_call() {
        let (mut io, cancel) = io_with(&[]);
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            c.store(true, Ordering::SeqCst);
        });
        let t = std::time::Instant::now();
        assert_eq!(io.call("terminal/wait_for_exit", json!({})), Err("cancelled".to_string()));
        assert!(t.elapsed() < Duration::from_millis(400));
    }

    const PROMPT_A: &str = "{\"method\":\"session/prompt\",\"id\":1}\n";
    const CANCEL: &str = "{\"method\":\"session/cancel\"}\n";
    const PROMPT_B: &str = "{\"method\":\"session/prompt\",\"id\":2}\n";

    /// Reader-backed Io that has already consumed `n_msgs` messages (synchronizes with the reader thread).
    fn reader_io(input: &str, n_msgs: usize) -> Io<Vec<u8>> {
        let cancel = Arc::new(AtomicBool::new(false));
        let inflight = Arc::new(AtomicUsize::new(0));
        let rx = spawn_reader(Cursor::new(input.to_string()), cancel.clone(), inflight.clone());
        let mut io = Io::new(rx, Vec::new(), cancel, inflight);
        for _ in 0..n_msgs {
            io.read_msg().unwrap().unwrap();
        }
        io
    }

    #[test]
    fn cancel_then_immediate_new_prompt_cancels_active_turn() {
        // Turn A is active; the client sends cancel and prompt B back-to-back before A checks the flag.
        let io = reader_io(&format!("{PROMPT_A}{CANCEL}{PROMPT_B}"), 3);
        assert!(io.cancelled(), "turn A still sees the cancel although prompt B was read after it");
        io.end_turn(); // A finishes as cancelled
        assert!(!io.cancelled(), "flag is not left set for B");
        assert_eq!(io.in_flight.load(Ordering::SeqCst), 1, "B is in flight and runs normally");
        io.end_turn();
        assert_eq!(io.in_flight.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancel_with_no_prompt_in_flight_is_ignored() {
        let io = reader_io(&format!("{CANCEL}{PROMPT_B}"), 2);
        assert!(!io.cancelled(), "a stale cancel must not cancel the next prompt");
    }

    #[test]
    fn reader_closes_at_eof_and_reports_bad_json() {
        let cancel = Arc::new(AtomicBool::new(false));
        let input = "\n{\"method\":\"initialize\",\"id\":1}\nnot json\n";
        let rx = spawn_reader(Cursor::new(input.to_string()), cancel, Arc::new(AtomicUsize::new(0)));
        assert_eq!(rx.recv().unwrap().unwrap()["method"], "initialize");
        assert!(rx.recv().unwrap().is_err());
        assert!(rx.recv().is_err());
    }
}
