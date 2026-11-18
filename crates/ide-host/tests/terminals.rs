#![cfg(windows)]
use ide_host::terminals::*;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::time::timeout;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn drain(rx: &mut UnboundedReceiver<TerminalEvent>) -> Vec<TerminalEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

async fn run_to_end(
    args: &[&str],
    cwd: &Path,
) -> (String, bool, ExitStatus, Vec<TerminalEvent>) {
    let (tx, mut rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let id = t.create("cmd", &s(args), cwd).expect("create");
    let st = timeout(Duration::from_secs(90), t.wait_for_exit(id))
        .await
        .expect("wait timed out")
        .expect("wait");
    let (out, trunc) = t.output(id).expect("output");
    (out, trunc, st, drain(&mut rx))
}

#[test]
fn ring_buffer_keeps_tail_and_flags_truncation() {
    let mut r = RingBuffer::new(8);
    r.push(b"abcd");
    assert_eq!(r.snapshot(), ("abcd".to_string(), false));
    r.push(b"efghij");
    assert_eq!(r.snapshot(), ("cdefghij".to_string(), true));
    r.push(&[b'x'; 100]);
    assert_eq!(r.snapshot(), ("xxxxxxxx".to_string(), true));
}

#[test]
fn ring_buffer_utf8_boundary_after_truncation() {
    // 'é' is 2 bytes; cap 5 over "aéééé" (9 bytes) keeps last 5 = [0xA9,é,é] -> partial lead dropped
    let mut r = RingBuffer::new(5);
    r.push("aéééé".as_bytes());
    let (s, t) = r.snapshot();
    assert!(t);
    assert_eq!(s, "éé");
    // invalid bytes inside are replaced, not an error
    let mut r = RingBuffer::new(16);
    r.push(&[b'a', 0xFF, b'b']);
    assert_eq!(r.snapshot().0, "a\u{FFFD}b");
}

#[tokio::test]
async fn agent_terminal_runs_and_reports_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, st, ev) = run_to_end(&["/c", "exit", "3"], dir.path()).await;
    assert_eq!(st.code, Some(3));
    assert!(ev.iter().any(|e| matches!(e, TerminalEvent::Exited { code: Some(3), .. })));
}

#[tokio::test]
async fn stderr_is_merged_into_output() {
    let dir = tempfile::tempdir().unwrap();
    let (out, _, _, _) = run_to_end(&["/c", "echo out& echo err 1>&2"], dir.path()).await;
    assert!(out.contains("out"), "{out:?}");
    assert!(out.contains("err"), "{out:?}");
}

#[tokio::test]
async fn cwd_is_respected() {
    let dir = tempfile::tempdir().unwrap();
    let (out, _, _, _) = run_to_end(&["/c", "cd"], dir.path()).await;
    let want = dunce::canonicalize(dir.path()).unwrap();
    let got = dunce::canonicalize(out.trim()).unwrap();
    assert_eq!(got, want);
}

#[tokio::test]
async fn stdin_is_closed_so_reading_commands_do_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    // findstr reads stdin; with a null stdin it ends immediately
    let (_, _, st, _) = run_to_end(&["/c", "findstr x"], dir.path()).await;
    assert!(st.code.is_some());
}

#[tokio::test]
async fn huge_output_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let (out, trunc, st, ev) = run_to_end(
        &["/c", "for /L %i in (1,1,120000) do @echo line%i"],
        dir.path(),
    )
    .await;
    assert_eq!(st.code, Some(0));
    assert!(out.len() <= AGENT_TERMINAL_BUFFER, "len {}", out.len());
    assert!(trunc);
    assert!(out.contains("line120000"), "tail must be kept");
    let mut emitted = 0usize;
    let mut truncs = 0;
    for e in &ev {
        match e {
            TerminalEvent::Output { chunk, .. } => {
                assert!(chunk.len() <= 32 * 1024);
                emitted += chunk.len();
            }
            TerminalEvent::Truncated { .. } => truncs += 1,
            _ => {}
        }
    }
    assert_eq!(truncs, 1);
    assert!(emitted <= 1024 * 1024 + 32 * 1024, "emitted {emitted}");
}

#[tokio::test]
async fn events_are_coalesced() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, _, ev) = run_to_end(
        &["/c", "for /L %i in (1,1,20000) do @echo line%i"],
        dir.path(),
    )
    .await;
    let n = ev
        .iter()
        .filter(|e| matches!(e, TerminalEvent::Output { .. }))
        .count();
    assert!(n > 0 && n * 10 < 20000, "{n} output events for 20000 writes");
}

#[tokio::test]
async fn release_kills_running_process() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let id = t
        .create("ping", &s(&["-n", "30", "127.0.0.1"]), dir.path())
        .unwrap();
    let started = std::time::Instant::now();
    t.release(id);
    // the child is reaped: an Exited event arrives well before ping would finish
    loop {
        let e = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no Exited after release")
            .expect("channel closed");
        if matches!(e, TerminalEvent::Exited { .. }) {
            break;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(matches!(t.wait_for_exit(id).await, Err(TerminalError::Unknown)));
    assert!(matches!(t.output(id), Err(TerminalError::Unknown)));
}

#[tokio::test]
async fn unknown_program_create_is_not_found() {
    let (tx, _rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let dir = tempfile::tempdir().unwrap();
    let r = t.create("definitely-not-a-real-program-xyz", &[], dir.path());
    assert!(matches!(r, Err(TerminalError::NotFound(_))));
}

#[tokio::test]
async fn user_pty_echoes_input() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = unbounded_channel();
    let pty = UserPty::spawn("cmd", dir.path(), 24, 80, tx).expect("spawn");
    pty.write(b"echo hi\r\n").unwrap();
    pty.resize(30, 100).unwrap();
    let mut buf = Vec::new();
    let found = timeout(Duration::from_secs(10), async {
        while let Some(ev) = rx.recv().await {
            if let PtyEvent::Output(b) = ev {
                buf.extend_from_slice(&b);
                // ConPTY asks the terminal for the cursor position (ESC[6n) and
                // holds output until answered; a real frontend (xterm.js) replies.
                if b.windows(4).any(|w| w == b"[6n") {
                    pty.write(b"[1;1R").unwrap();
                }
                if String::from_utf8_lossy(&buf).contains("hi") {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(found, "got {:?}", String::from_utf8_lossy(&buf));
    pty.kill();
}
