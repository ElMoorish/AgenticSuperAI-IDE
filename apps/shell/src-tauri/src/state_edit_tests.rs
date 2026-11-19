//! Task 11: the ApplyEdit / confirm_edit round trip, driven against the real agent process
//! (no model server is needed: the agent just sits idle and ignores responses it did not ask for).

use super::*;
use ide_host::editor::PendingEdit;
use serde_json::json;

fn collector() -> (Sink, Arc<Mutex<Vec<UiEvent>>>) {
    let got = Arc::new(Mutex::new(Vec::<UiEvent>::new()));
    let g2 = got.clone();
    let sink: Sink = Arc::new(move |e: &UiEvent| {
        g2.lock().unwrap().push(e.clone());
        true
    });
    (sink, got)
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// An AppState with the real agent running in a temp folder holding a.txt = "old\n".
async fn fixture(timeout_ms: u64) -> (Arc<AppState>, tempfile::TempDir, Arc<Mutex<Vec<UiEvent>>>) {
    let exe = super::tests::agent_exe_or_build();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
    let mut st = AppState::new(dir.path().join("cfg"), Some(exe));
    st.confirm_timeout = Duration::from_millis(timeout_ms);
    let st = Arc::new(st);
    let (sink, got) = collector();
    st.set_sink(sink);
    tokio::spawn(run_pump(st.take_event_rx().unwrap(), st.sink_slot()));
    st.open_folder(dir.path().to_str().unwrap()).await.unwrap();
    (st, dir, got)
}

fn register(st: &AppState, path: &str, new: &str) -> (u64, String, bool) {
    let ctx = st.ctx().unwrap();
    let (old, stamp) = read_for_approval(&ctx.ws, path).unwrap();
    let existed = stamp.is_some();
    let pe = PendingEdit { id: 0, agent_id: json!(4242), path: path.into(), old: old.clone(), new: new.into() };
    (ctx.register_approval(Pending::Edit { pe, stamp }), old, existed)
}

fn read(dir: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(dir.path().join(name)).unwrap()
}

#[tokio::test]
async fn accepted_edit_is_handed_to_the_ui_and_not_written_by_the_host() {
    let (st, dir, got) = fixture(60_000).await;
    let (id, old, _) = register(&st, "a.txt", "new\n");
    st.resolve_approval(id, true).await.unwrap();
    settle().await;
    let evs = got.lock().unwrap().clone();
    assert!(
        evs.contains(&UiEvent::ApplyEdit { id, rel: "a.txt".into(), old, new_text: "new\n".into() }),
        "{evs:?}"
    );
    assert_eq!(read(&dir, "a.txt"), "old\n", "the host must not write while the UI owns the edit");
    st.confirm_edit(id, true, None).unwrap();
    assert_eq!(st.confirm_edit(id, true, None).unwrap_err().code, "unknown_approval", "confirm answers once");
    st.shutdown().await;
}

#[tokio::test]
async fn awaiting_edit_is_replayed_to_a_resubscribing_ui() {
    let (st, _dir, _got) = fixture(60_000).await;
    let (id, _, _) = register(&st, "a.txt", "new\n");
    st.resolve_approval(id, true).await.unwrap();
    settle().await;
    let (sink2, got2) = collector();
    st.subscribe(sink2);
    settle().await;
    assert!(
        got2.lock().unwrap().iter().any(|e| matches!(e, UiEvent::ApplyEdit { id: i, .. } if *i == id)),
        "a reloaded webview must be able to finish the edit"
    );
    st.confirm_edit(id, false, Some("declined".into())).unwrap();
    let (sink3, got3) = collector();
    st.subscribe(sink3);
    settle().await;
    assert!(!got3.lock().unwrap().iter().any(|e| matches!(e, UiEvent::ApplyEdit { .. })), "confirmed edits are not replayed");
    st.shutdown().await;
}

#[tokio::test]
async fn unconfirmed_edit_is_written_by_the_host_after_the_timeout() {
    let (st, dir, _got) = fixture(150).await;
    let (id, _, _) = register(&st, "a.txt", "fallback\n");
    st.resolve_approval(id, true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(read(&dir, "a.txt"), "fallback\n");
    assert_eq!(st.confirm_edit(id, true, None).unwrap_err().code, "unknown_approval", "a late confirm answers nothing");
    st.shutdown().await;
}

#[tokio::test]
async fn fallback_write_is_stamp_checked_and_never_clobbers_a_changed_file() {
    let (st, dir, _got) = fixture(150).await;
    let (id, _, _) = register(&st, "a.txt", "fallback\n");
    st.resolve_approval(id, true).await.unwrap();
    std::fs::write(dir.path().join("a.txt"), "someone else wrote this\n").unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(read(&dir, "a.txt"), "someone else wrote this\n");
    st.shutdown().await;
}

#[tokio::test]
async fn ui_confirmation_cancels_the_fallback_write() {
    let (st, dir, _got) = fixture(300).await;
    let (id, _, _) = register(&st, "a.txt", "new\n");
    st.resolve_approval(id, true).await.unwrap();
    st.confirm_edit(id, true, None).unwrap(); // the UI reports it applied the edit (nothing is written in this test)
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(read(&dir, "a.txt"), "old\n", "no second write by the host after the UI answered");
    st.shutdown().await;
}

#[tokio::test]
async fn accepted_new_file_is_created_by_the_host_without_a_round_trip() {
    let (st, dir, got) = fixture(60_000).await;
    let (id, old, existed) = register(&st, "fresh.txt", "created\n");
    assert!(old.is_empty() && !existed);
    st.resolve_approval(id, true).await.unwrap();
    assert_eq!(read(&dir, "fresh.txt"), "created\n");
    settle().await;
    assert!(!got.lock().unwrap().iter().any(|e| matches!(e, UiEvent::ApplyEdit { .. })));
    st.shutdown().await;
}

#[tokio::test]
async fn rejected_edit_and_stale_ids_never_reach_the_ui() {
    let (st, dir, got) = fixture(60_000).await;
    let (id, _, _) = register(&st, "a.txt", "new\n");
    st.resolve_approval(id, false).await.unwrap();
    settle().await;
    assert!(!got.lock().unwrap().iter().any(|e| matches!(e, UiEvent::ApplyEdit { .. })));
    assert_eq!(read(&dir, "a.txt"), "old\n");
    assert_eq!(st.resolve_approval(id, true).await.unwrap_err().code, "unknown_approval");
    assert_eq!(st.confirm_edit(id, true, None).unwrap_err().code, "unknown_approval");
    st.shutdown().await;
}

#[tokio::test]
async fn agent_cancel_forgets_edits_waiting_for_the_ui() {
    let (st, dir, _got) = fixture(200).await;
    let (id, _, _) = register(&st, "a.txt", "new\n");
    st.resolve_approval(id, true).await.unwrap();
    st.agent_cancel();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(read(&dir, "a.txt"), "old\n", "after a cancel the agent no longer waits, so the host must not write");
    assert_eq!(st.confirm_edit(id, true, None).unwrap_err().code, "unknown_approval");
    st.shutdown().await;
}
