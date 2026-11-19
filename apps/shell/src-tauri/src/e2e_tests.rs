//! Task 15: the whole host path, headless. Real AppState + real agent-core process + a scripted
//! OpenAI-compatible model server on localhost. The UI's part (applying an accepted edit to the
//! editor buffer, saving it, confirming) is played by the test through the same host commands.
//!
//! Path covered: prompt -> model streams a tool call -> approval -> accept/reject/cancel ->
//! file on disk -> the agent's follow-up request -> final text -> TurnEnd.

use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

const EDIT_DIFF: &str = "<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n";

/// Answers /v1/chat/completions: with a tool call until a tool result is in the conversation, then
/// with plain text. Counts requests so a test can see the agent asked again.
struct MockModel {
    url: String,
    requests: Arc<AtomicU64>,
}

fn sse(chunks: &[Value]) -> String {
    let mut s = String::new();
    for c in chunks {
        s.push_str(&format!("data: {c}\n\n"));
    }
    s.push_str("data: [DONE]\n\n");
    s
}

fn start_mock_model() -> MockModel {
    start_mock_model_with(None)
}

/// `nav`: Some((tool, args)) makes the model call that read-only tool first and then repeat the
/// tool result back in its answer, so a test can see what the host returned.
fn start_mock_model_with(nav: Option<(&'static str, Value)>) -> MockModel {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicU64::new(0));
    let r2 = requests.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let r3 = r2.clone();
            let nav = nav.clone();
            std::thread::spawn(move || {
                let mut rd = BufReader::new(conn.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if rd.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                if rd.read_exact(&mut body).is_err() {
                    return;
                }
                r3.fetch_add(1, Ordering::SeqCst);
                let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let has_tool_result = req["messages"].as_array().is_some_and(|m| {
                    m.iter().any(|x| x["role"] == "tool" || x["content"].as_str().is_some_and(|c| c.contains("<tool_result>")))
                });
                let last_result = req["messages"].as_array().and_then(|m| {
                    m.iter().rev().find_map(|x| {
                        let c = x["content"].as_str()?;
                        (x["role"] == "tool" || c.contains("<tool_result>")).then(|| c.to_string())
                    })
                });
                let payload = if let (Some((tool, args)), false) = (&nav, has_tool_result) {
                    sse(&[json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call1","function":{"name":tool,"arguments":args.to_string()}}]}}]})])
                } else if nav.is_some() {
                    sse(&[json!({"choices":[{"delta":{"content":format!("RESULT<<{}>>", last_result.unwrap_or_default())}}]})])
                } else if has_tool_result {
                    sse(&[json!({"choices":[{"delta":{"content":"All done."}}]})])
                } else {
                    let args = json!({"path": "a.txt", "diff": EDIT_DIFF}).to_string();
                    sse(&[json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call1","function":{"name":"edit_file_search_replace","arguments":args}}]}}]})])
                };
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(payload.as_bytes());
            });
        }
    });
    MockModel { url, requests }
}

type Events = Arc<Mutex<Vec<UiEvent>>>;

async fn fixture(model: &MockModel) -> (Arc<AppState>, tempfile::TempDir, Events) {
    let exe = super::tests::agent_exe_or_build();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
    let mut st = AppState::new(dir.path().join("cfg"), Some(exe));
    st.confirm_timeout = Duration::from_millis(500);
    let st = Arc::new(st);
    let mut s = st.get_settings();
    s.endpoint = model.url.clone();
    st.set_settings(s).await.unwrap();
    let got: Events = Arc::new(Mutex::new(Vec::new()));
    let g2 = got.clone();
    st.set_sink(Arc::new(move |e: &UiEvent| {
        g2.lock().unwrap().push(e.clone());
        true
    }));
    tokio::spawn(run_pump(st.take_event_rx().unwrap(), st.sink_slot()));
    st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
    (st, dir, got)
}

async fn wait_for<T>(got: &Events, f: impl Fn(&UiEvent) -> Option<T>) -> T {
    for _ in 0..300 {
        let hit = got.lock().unwrap().iter().find_map(&f);
        if let Some(v) = hit {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for an event; got {:?}", got.lock().unwrap());
}

fn read(dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(dir.path().join("a.txt")).unwrap()
}

fn start_prompt(st: &Arc<AppState>) -> tokio::task::JoinHandle<Result<(), AppError>> {
    let st = st.clone();
    tokio::spawn(async move { st.agent_prompt("build", "change old to new in a.txt").await })
}

#[tokio::test]
async fn prompt_edit_approve_apply_confirm_turn_end() {
    let model = start_mock_model();
    let (st, dir, got) = fixture(&model).await;
    let turn = start_prompt(&st);

    let id = wait_for(&got, |e| match e {
        UiEvent::Approval { id, kind, title, old, new, .. } if kind == "edit" => {
            assert_eq!(title, "a.txt");
            assert_eq!(old.as_deref(), Some("old\n"));
            assert_eq!(new.as_deref(), Some("new\n"));
            Some(*id)
        }
        _ => None,
    })
    .await;
    assert_eq!(read(&dir), "old\n", "nothing is written before the user decides");

    st.resolve_approval(id, true).await.unwrap();
    let (rel, old, new_text) = wait_for(&got, |e| match e {
        UiEvent::ApplyEdit { id: i, rel, old, new_text } if *i == id => Some((rel.clone(), old.clone(), new_text.clone())),
        _ => None,
    })
    .await;
    assert_eq!((rel.as_str(), old.as_str(), new_text.as_str()), ("a.txt", "old\n", "new\n"));
    assert_eq!(read(&dir), "old\n", "the UI owns the write until it confirms");

    // the UI applies the edit to its buffer, saves through the stamp-checked path and confirms
    let cur = st.read_file("a.txt").unwrap();
    st.save_file("a.txt", &new_text, Some(&cur.stamp)).unwrap();
    st.confirm_edit(id, true, None).unwrap();

    turn.await.unwrap().unwrap();
    assert_eq!(read(&dir), "new\n");
    wait_for(&got, |e| matches!(e, UiEvent::TurnEnd { .. }).then_some(())).await;
    let evs = got.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(e, UiEvent::ToolCall { .. })), "tool events reach the UI: {evs:?}");
    let text: String = evs.iter().filter_map(|e| if let UiEvent::Chunk { text } = e { Some(text.as_str()) } else { None }).collect();
    assert!(text.contains("All done."), "the follow-up answer streams to the UI: {text:?}");
    assert!(evs.iter().any(|e| matches!(e, UiEvent::TurnEnd { stop_reason } if stop_reason == "end_turn")), "{evs:?}");
    assert!(model.requests.load(Ordering::SeqCst) >= 2, "the agent asked the model again after the tool result");
    st.shutdown().await;
}

#[tokio::test]
async fn rejected_edit_leaves_the_file_alone_and_the_turn_still_ends() {
    let model = start_mock_model();
    let (st, dir, got) = fixture(&model).await;
    let turn = start_prompt(&st);
    let id = wait_for(&got, |e| if let UiEvent::Approval { id, .. } = e { Some(*id) } else { None }).await;
    st.resolve_approval(id, false).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(15), turn).await.expect("turn ended");
    assert_eq!(read(&dir), "old\n");
    wait_for(&got, |e| matches!(e, UiEvent::TurnEnd { .. }).then_some(())).await;
    assert!(!got.lock().unwrap().iter().any(|e| matches!(e, UiEvent::ApplyEdit { .. })));
    st.shutdown().await;
}

#[tokio::test]
async fn cancel_while_waiting_for_approval_ends_the_turn_cancelled() {
    let model = start_mock_model();
    let (st, dir, got) = fixture(&model).await;
    let turn = start_prompt(&st);
    wait_for(&got, |e| if let UiEvent::Approval { id, .. } = e { Some(*id) } else { None }).await;
    st.agent_cancel();
    let _ = tokio::time::timeout(Duration::from_secs(15), turn).await.expect("turn ended after cancel");
    assert_eq!(read(&dir), "old\n");
    let reason = wait_for(&got, |e| if let UiEvent::TurnEnd { stop_reason } = e { Some(stop_reason.clone()) } else { None }).await;
    assert_eq!(reason, "cancelled");
    // the busy flag is free again
    let again = st.agent_prompt("plan", "next").await;
    assert_ne!(again.err().map(|e| e.code), Some("busy".to_string()));
    st.shutdown().await;
}

#[tokio::test]
async fn a_file_changed_after_the_proposal_is_not_clobbered() {
    let model = start_mock_model();
    let (st, dir, got) = fixture(&model).await;
    let turn = start_prompt(&st);
    let id = wait_for(&got, |e| if let UiEvent::Approval { id, .. } = e { Some(*id) } else { None }).await;
    std::fs::write(dir.path().join("a.txt"), "someone else wrote this\n").unwrap();
    let _ = st.resolve_approval(id, true).await; // may be refused outright (conflict) or fail the edit
    let r = tokio::time::timeout(Duration::from_secs(20), turn).await.expect("the turn ends once the host gives up on the stale edit");
    let _ = r;
    assert_eq!(read(&dir), "someone else wrote this\n", "the newer on-disk text must survive");
    st.shutdown().await;
}

async fn run_nav(tool: &'static str, args: Value, mode: &str) -> String {
    let model = start_mock_model_with(Some((tool, args)));
    let (st, _dir, got) = fixture(&model).await;
    let st2 = st.clone();
    let mode = mode.to_string();
    let turn = tokio::spawn(async move { st2.agent_prompt(&mode, "look around").await });
    let _ = tokio::time::timeout(Duration::from_secs(20), turn).await.expect("turn ended");
    wait_for(&got, |e| matches!(e, UiEvent::TurnEnd { .. }).then_some(())).await;
    let text: String = got
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| if let UiEvent::Chunk { text } = e { Some(text.clone()) } else { None })
        .collect();
    st.shutdown().await;
    text
}

#[tokio::test]
async fn agent_can_list_find_and_search_the_project_in_both_modes() {
    let listed = run_nav("list_dir", json!({"path": "."}), "plan").await;
    assert!(listed.contains("a.txt (4 bytes)"), "{listed}");
    let found = run_nav("find_files", json!({"query": "a.t"}), "build").await;
    assert!(found.contains("a.txt"), "{found}");
    let hits = run_nav("search_text", json!({"query": "OLD"}), "plan").await;
    assert!(hits.contains("a.txt:1: old"), "{hits}");
    // a missing folder is an ordinary tool error the model can read, not a crash
    let missing = run_nav("list_dir", json!({"path": "nope"}), "plan").await;
    assert!(missing.contains("not found"), "{missing}");
}
