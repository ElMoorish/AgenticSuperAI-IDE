#![cfg(windows)]
use ide_host::terminals::*;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, UnboundedReceiver};
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
const CPR_QUERY: &str = "\x1b[6n";
const CPR_REPLY: &[u8] = b"\x1b[1;1R";

/// Collects pty output until `needle` appears, answering ConPTY's cursor
/// position queries (as a real terminal frontend does). The query is detected
/// on the accumulated buffer, since a sequence can be split across reads.
async fn collect_until(
    pty: &UserPty,
    rx: &mut Receiver<PtyEvent>,
    buf: &mut Vec<u8>,
    needle: &str,
    secs: u64,
) -> bool {
    let mut replied = String::from_utf8_lossy(buf).matches(CPR_QUERY).count();
    timeout(Duration::from_secs(secs), async {
        loop {
            let text = String::from_utf8_lossy(buf).into_owned();
            let queries = text.matches(CPR_QUERY).count();
            while replied < queries {
                pty.write(CPR_REPLY).unwrap();
                replied += 1;
            }
            if text.contains(needle) {
                return true;
            }
            match rx.recv().await {
                Some(PtyEvent::Output(b)) => buf.extend_from_slice(&b),
                Some(PtyEvent::Exited(_)) => {}
                None => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

#[tokio::test]
async fn user_pty_echoes_input() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = channel(PTY_CHANNEL_CAPACITY);
    let pty = UserPty::spawn("cmd", dir.path(), 24, 80, tx).expect("spawn");
    pty.resize(30, 100).unwrap();
    // %X% is expanded when the command runs, so ASAOKDONE appears only in output
    // (the echoed input contains the literal "%X%DONE").
    pty.write(b"set X=ASAOK\r\n").unwrap();
    pty.write(b"echo %X%DONE\r\n").unwrap();
    let mut buf = Vec::new();
    let found = collect_until(&pty, &mut rx, &mut buf, "ASAOKDONE", 10).await;
    assert!(found, "got {:?}", String::from_utf8_lossy(&buf));
    pty.kill();
}

#[test]
fn user_pty_rejects_zero_size() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = channel(PTY_CHANNEL_CAPACITY);
    assert!(matches!(
        UserPty::spawn("cmd", dir.path(), 0, 80, tx),
        Err(TerminalError::InvalidSize)
    ));
}

#[tokio::test]
async fn slow_consumer_backpressures_pty_without_unbounded_growth() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = channel(PTY_CHANNEL_CAPACITY);
    let probe = tx.clone();
    assert_eq!(probe.max_capacity(), PTY_CHANNEL_CAPACITY);
    let pty = UserPty::spawn("cmd", dir.path(), 24, 80, tx).expect("spawn");
    let mut buf = Vec::new();
    pty.write(b"set X=END\r\n").unwrap();
    assert!(collect_until(&pty, &mut rx, &mut buf, "X=END", 10).await);
    pty.write(b"for /L %i in (1,1,40000) do @echo line%i padpadpadpadpadpadpadpadpad\r\n")
        .unwrap();
    pty.write(b"echo %X%MARK\r\n").unwrap();
    // Stop consuming: the bounded channel must fill up (producer blocks) rather than grow.
    let filled = timeout(Duration::from_secs(60), async {
        while probe.capacity() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(filled.is_ok(), "channel never filled; no backpressure observed");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let buffered = probe.max_capacity() - probe.capacity();
    assert!(buffered <= PTY_CHANNEL_CAPACITY, "buffered {buffered}");
    // Consume slowly; nothing is lost and the buffer never exceeds capacity.
    let done = timeout(Duration::from_secs(120), async {
        let mut n = 0u32;
        loop {
            let b = probe.max_capacity() - probe.capacity();
            assert!(b <= PTY_CHANNEL_CAPACITY, "buffered {b}");
            if collect_until(&pty, &mut rx, &mut buf, "ENDMARK", 1).await {
                return true;
            }
            n += 1;
            if n > 100_000 {
                return false;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(done, "ENDMARK never seen");
    pty.kill();
}

// ---------------------------------------------------------------- fix round 1

fn ping_count(marker: &str) -> usize {
    // Counts PING.EXE processes whose command line contains `marker`.
    let script = format!(
        "(Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*{marker}*' }} | Measure-Object).Count"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

#[tokio::test]
async fn too_many_live_terminals_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let mut ids = Vec::new();
    for _ in 0..MAX_LIVE_TERMINALS {
        ids.push(t.create("ping", &s(&["-n", "30", "127.0.0.1"]), dir.path()).unwrap());
    }
    let r = t.create("ping", &s(&["-n", "30", "127.0.0.1"]), dir.path());
    assert!(matches!(r, Err(TerminalError::TooMany)), "{r:?}");
    assert!(TerminalError::TooMany.to_string().contains("too many"));
    for id in ids {
        t.release(id);
    }
}

#[tokio::test]
async fn old_exited_terminals_are_evicted() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let mut ids = Vec::new();
    for _ in 0..(MAX_RETAINED_TERMINALS + 1) {
        let id = t.create("cmd", &s(&["/c", "exit", "0"]), dir.path()).unwrap();
        timeout(Duration::from_secs(10), t.wait_for_exit(id)).await.unwrap().unwrap();
        ids.push(id);
    }
    assert!(matches!(t.output(ids[0]), Err(TerminalError::Unknown)));
    assert!(matches!(t.wait_for_exit(ids[0]).await, Err(TerminalError::Unknown)));
    assert!(t.output(*ids.last().unwrap()).is_ok());
    assert!(t.output(ids[1]).is_ok());
}

#[tokio::test]
async fn release_after_exit_is_harmless() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let id = t.create("cmd", &s(&["/c", "exit", "0"]), dir.path()).unwrap();
    t.wait_for_exit(id).await.unwrap();
    t.release(id);
    t.release(id);
    t.release(9999);
    assert!(matches!(t.wait_for_exit(id).await, Err(TerminalError::Unknown)));
}

#[tokio::test]
async fn grandchild_holding_pipe_does_not_delay_or_reorder_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    let started = std::time::Instant::now();
    // cmd exits at once; the detached ping inherits stdout and keeps the pipe open ~7 s
    let id = t
        .create("cmd", &s(&["/c", "start /b ping -n 8 127.0.0.1 & exit 0"]), dir.path())
        .unwrap();
    loop {
        let e = timeout(Duration::from_secs(6), rx.recv())
            .await
            .expect("no Exited")
            .expect("closed");
        if matches!(e, TerminalEvent::Exited { .. }) {
            break;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    // nothing may be emitted after Exited, even while the grandchild keeps writing
    let late = timeout(Duration::from_millis(2500), rx.recv()).await;
    assert!(late.is_err(), "event after Exited: {late:?}");
    assert_eq!(t.wait_for_exit(id).await.unwrap().code, Some(0));
    let len_then = t.output(id).unwrap().0.len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(t.output(id).unwrap().0.len(), len_then, "ring kept filling after Exited");
}

#[tokio::test]
async fn release_kills_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = unbounded_channel();
    let t = AgentTerminals::new(tx);
    // cmd -> ping child; 63 is a unique marker in the grandchild's command line
    let id = t.create("cmd", &s(&["/c", "ping -n 63 127.0.0.1"]), dir.path()).unwrap();
    let up = timeout(Duration::from_secs(30), async {
        loop {
            if ping_count("-n 63 ") >= 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await;
    assert!(up.is_ok(), "grandchild never observed running");
    let at = std::time::Instant::now();
    t.release(id);
    loop {
        let e = timeout(Duration::from_secs(5), rx.recv()).await.expect("no Exited").unwrap();
        if matches!(e, TerminalEvent::Exited { .. }) {
            break;
        }
    }
    // pipes were released promptly (a surviving grandchild would delay Exited by the 2 s drain)
    assert!(at.elapsed() < Duration::from_millis(1800), "{:?}", at.elapsed());
    let gone = timeout(Duration::from_secs(10), async {
        loop {
            if ping_count("-n 63 ") == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "grandchild ping survived release");
}

#[tokio::test]
async fn shim_arguments_with_metacharacters_are_not_interpreted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("echoargs.cmd"), "@echo off\r\necho ARGS[%*]\r\n").unwrap();
    let marker = dir.path().join("marker.txt");
    let cases = [
        "a&b",
        "x & echo INJECTED > marker.txt",
        "p%PATH%q",
        "c^d",
        "e|f",
        "q\"x",
        "q\" & echo INJECTED > marker.txt & \"",
    ];
    let mut ran = 0;
    for arg in cases {
        let (tx, _rx) = unbounded_channel();
        let t = AgentTerminals::new(tx);
        // bare name: resolved through the injected PATH-independent full path
        let shim = dir.path().join("echoargs.cmd");
        let r = t.create(shim.to_str().unwrap(), &[arg.to_string()], dir.path());
        match r {
            // std refuses batch args it cannot escape safely: an error, not a panic
            Err(TerminalError::Io(_)) => continue,
            Err(e) => panic!("{arg:?}: {e:?}"),
            Ok(id) => {
                ran += 1;
                timeout(Duration::from_secs(10), t.wait_for_exit(id)).await.unwrap().unwrap();
                let out = t.output(id).unwrap().0;
                assert!(!marker.exists(), "injection executed for {arg:?}: {out:?}");
                if !arg.contains('%') && !arg.contains('"') {
                    assert!(out.contains(arg), "{arg:?} not literal in {out:?}");
                }
                if arg.contains('%') {
                    assert!(!out.contains("C:\\") && !out.contains("Windows"), "expanded: {out:?}");
                }
            }
        }
    }
    assert!(ran > 0, "every case was rejected");
    assert!(!marker.exists());
}

#[tokio::test]
async fn invalid_utf8_chunks_stay_within_32kb() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("junk.bin");
    std::fs::write(&file, vec![0xFFu8; 120_000]).unwrap();
    let (_, _, _, ev) = run_to_end(&["/c", "type", file.to_str().unwrap()], dir.path()).await;
    let mut total = 0;
    for e in &ev {
        if let TerminalEvent::Output { chunk, .. } = e {
            assert!(chunk.len() <= 32 * 1024, "chunk {}", chunk.len());
            total += chunk.len();
        }
    }
    assert!(total > 100_000, "total {total}");
}
