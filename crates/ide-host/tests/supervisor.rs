use ide_host::supervisor::EventRx;
use ide_host::{AcpError, AgentConfig, AgentEvent, Supervisor};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

fn cfg(mode: &str) -> AgentConfig {
    AgentConfig {
        exe: env!("CARGO_BIN_EXE_mock_agent").into(),
        env: vec![("MOCK_MODE".into(), mode.into())],
        cwd: std::env::temp_dir(),
        backoff: vec![Duration::from_millis(10), Duration::from_millis(20)],
        max_crashes_in_window: 5,
        crash_window: Duration::from_secs(60),
    }
}

async fn next(rx: &mut EventRx) -> AgentEvent {
    timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event stream ended")
}

#[tokio::test]
async fn routes_response_to_waiting_request() {
    let (sup, _rx) = Supervisor::start(cfg("echo"));
    let a = sup.request("m/a", json!({"n":1}));
    let b = sup.request("m/b", json!({"n":2}));
    let (a, b) = tokio::join!(a, b);
    assert_eq!(a.unwrap(), json!({"echo":{"n":1}}));
    assert_eq!(b.unwrap(), json!({"echo":{"n":2}}));
    sup.shutdown().await;
}

#[tokio::test]
async fn forwards_agent_requests_as_events() {
    let (sup, mut rx) = Supervisor::start(cfg("ask"));
    let sup = std::sync::Arc::new(sup);
    let s2 = sup.clone();
    let req = tokio::spawn(async move { s2.request("session/prompt", json!({})).await });
    match next(&mut rx).await {
        AgentEvent::Request { id, method, params } => {
            assert_eq!(id, json!(1000));
            assert_eq!(method, "fs/read_text_file");
            assert_eq!(params, json!({"path":"a.txt"}));
            sup.respond(id, Ok(json!({"content":"hi"})));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(req.await.unwrap().unwrap(), json!({"asked":true}));
    sup.shutdown().await;
}

#[tokio::test]
async fn pending_requests_fail_on_exit() {
    let (sup, mut rx) = Supervisor::start(cfg("exit-after=300"));
    let r = timeout(Duration::from_secs(10), sup.request("m", json!({})))
        .await
        .expect("request hung");
    assert_eq!(r, Err(AcpError::Exited));
    assert!(matches!(next(&mut rx).await, AgentEvent::Exited { code: Some(0) }));
    sup.shutdown().await;
}

#[tokio::test]
async fn restarts_with_backoff_then_gives_up() {
    let (sup, mut rx) = Supervisor::start(cfg("crash"));
    let mut restarts = vec![];
    loop {
        match next(&mut rx).await {
            AgentEvent::Exited { code } => assert_eq!(code, Some(3)),
            AgentEvent::Restarted { attempt } => restarts.push(attempt),
            AgentEvent::GaveUp => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(restarts, vec![1, 2, 3, 4]);
    assert_eq!(sup.request("m", json!({})).await, Err(AcpError::Exited));
    sup.shutdown().await;
}

#[tokio::test]
async fn later_requests_go_to_new_process() {
    let (sup, mut rx) = Supervisor::start(cfg("echo,exit-after=400"));
    assert_eq!(sup.request("m", json!(1)).await.unwrap(), json!({"echo":1}));
    loop {
        if let AgentEvent::Restarted { attempt: 1 } = next(&mut rx).await {
            break;
        }
    }
    assert_eq!(sup.request("m", json!(2)).await.unwrap(), json!({"echo":2}));
    sup.shutdown().await;
}

#[tokio::test]
async fn splits_lines_with_utf8_and_long_lines() {
    let (sup, mut rx) = Supervisor::start(cfg("longline,echo"));
    match next(&mut rx).await {
        AgentEvent::Update { session_id, update } => {
            assert_eq!(session_id, "s1");
            let t = update["text"].as_str().unwrap();
            assert!(t.len() > 1_000_000);
            assert!(t.starts_with("h\u{e9}llo\u{1F600}"));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(sup.request("m", json!("\u{fc}")).await.unwrap(), json!({"echo":"\u{fc}"}));
    sup.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_stream_without_gave_up() {
    let (sup, mut rx) = Supervisor::start(cfg("echo"));
    sup.request("m", json!(0)).await.unwrap();
    sup.shutdown().await;
    assert!(timeout(Duration::from_secs(5), rx.recv()).await.unwrap().is_none());
    assert_eq!(sup.request("m", json!(0)).await, Err(AcpError::Closed));
}

#[tokio::test]
async fn response_routed_while_consumer_not_draining() {
    let (sup, mut rx) = Supervisor::start(cfg("echo,flood=1000"));
    let r = timeout(Duration::from_secs(2), sup.request("m", json!(5)))
        .await
        .expect("request deadlocked");
    assert_eq!(r.unwrap(), json!({"echo":5}));
    let mut n = 0;
    while n < 1000 {
        if let AgentEvent::Update { .. } = next(&mut rx).await {
            n += 1;
        }
    }
    sup.shutdown().await;
}

#[tokio::test]
async fn shutdown_completes_while_consumer_not_draining() {
    let (sup, _rx) = Supervisor::start(cfg("echo,flood=1000,exit-after=200"));
    sup.request("m", json!(1)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await; // exits + restarts, events pile up
    timeout(Duration::from_secs(5), sup.shutdown()).await.expect("shutdown hung");
}

#[tokio::test]
async fn dropped_request_removes_pending_entry() {
    let (sup, _rx) = Supervisor::start(cfg("silent"));
    let r = timeout(Duration::from_millis(200), sup.request("m", json!({}))).await;
    assert!(r.is_err());
    assert_eq!(sup.pending_len(), 0);
    sup.shutdown().await;
}

#[tokio::test]
async fn string_id_agent_request_is_forwarded_and_answered() {
    let (sup, mut rx) = Supervisor::start(cfg("askstr"));
    let sup = std::sync::Arc::new(sup);
    let s2 = sup.clone();
    let req = tokio::spawn(async move { s2.request("session/prompt", json!({})).await });
    match next(&mut rx).await {
        AgentEvent::Request { id, method, .. } => {
            assert_eq!(id, json!("abc"));
            assert_eq!(method, "fs/read_text_file");
            sup.respond(id, Ok(json!({"content":"hi"})));
        }
        other => panic!("unexpected {other:?}"),
    }
    // the mock only answers our request once it saw the response carrying id "abc"
    assert_eq!(req.await.unwrap().unwrap(), json!({"asked":true}));
    sup.shutdown().await;
}

#[tokio::test]
async fn no_lifecycle_events_after_shutdown() {
    // shutdown while running (mock exits at 100ms, shutdown at ~50ms)
    let (sup, mut rx) = Supervisor::start(cfg("exit-after=100"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    sup.shutdown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    if let Ok(Some(ev)) = timeout(Duration::from_secs(2), rx.recv()).await {
        panic!("event after shutdown: {ev:?}");
    }
    // shutdown during backoff
    let mut c = cfg("crash");
    c.backoff = vec![Duration::from_secs(30)];
    let (sup, mut rx) = Supervisor::start(c);
    assert!(matches!(next(&mut rx).await, AgentEvent::Exited { .. }));
    sup.shutdown().await;
    assert!(timeout(Duration::from_secs(2), rx.recv()).await.unwrap().is_none());
}
