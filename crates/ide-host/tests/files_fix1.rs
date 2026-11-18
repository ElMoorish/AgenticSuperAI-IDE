use ide_host::editor::{BufferView, EditorHandler, HandlerOutcome};
use ide_host::files::{read_stamped, write_atomic_hooked, write_atomic_with, WriteError};
use ide_host::Workspace;
use serde_json::json;
use std::fs;
use std::path::Path;

struct NoBufs;
impl BufferView for NoBufs {
    fn open_buffer(&self, _abs: &Path) -> Option<String> {
        None
    }
}

fn ws() -> (tempfile::TempDir, Workspace) {
    let d = tempfile::tempdir().unwrap();
    let w = Workspace::open(d.path()).unwrap();
    (d, w)
}

#[test]
fn conflict_detected_between_first_check_and_rename() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "one").unwrap();
    let (_, s) = read_stamped(&w, "a.txt").unwrap();
    let target = d.path().join("a.txt");
    let hook = || fs::write(&target, "external").unwrap();
    let r = write_atomic_hooked(&w, "a.txt", "mine", Some(&s), false, Some(&hook));
    assert!(matches!(r, Err(WriteError::Conflict)));
    assert_eq!(fs::read_to_string(&target).unwrap(), "external");
    let names: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.txt".to_string()]);
}

#[test]
fn failed_write_does_not_leave_new_empty_directories() {
    let (d, w) = ws();
    fs::create_dir(d.path().join("keep")).unwrap();
    assert!(write_atomic_with(&w, "a/b/c.txt", "x", None, true).is_err());
    assert!(!d.path().join("a").exists());
    assert!(write_atomic_with(&w, "keep/n/c.txt", "x", None, true).is_err());
    assert!(!d.path().join("keep/n").exists());
    assert!(d.path().join("keep").is_dir());
}

fn pending(w: &Workspace, path: &str, content: &str) -> (String, String) {
    let h = EditorHandler::new(w.clone(), NoBufs);
    match h.handle(
        json!(1),
        "fs/write_text_file",
        &json!({"path":path,"content":content}),
    ) {
        HandlerOutcome::NeedsApproval(e) => (e.old, e.new),
        o => panic!("{o:?}"),
    }
}

#[test]
fn agent_edit_on_crlf_file_keeps_crlf() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), b"a\r\nb\r\n").unwrap();
    let (old, new) = pending(&w, "a.txt", "a\nB\nc\n");
    assert_eq!(old, "a\r\nb\r\n");
    assert_eq!(new, "a\r\nB\r\nc\r\n");
}

#[test]
fn agent_edit_on_lf_file_keeps_lf() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), b"a\nb\n").unwrap();
    let (old, new) = pending(&w, "a.txt", "a\r\nB\r\n");
    assert_eq!(old, "a\nb\n");
    assert_eq!(new, "a\nB\n");
}

#[test]
fn agent_write_new_file_keeps_agent_text() {
    let (_d, w) = ws();
    let (old, new) = pending(&w, "n.txt", "x\r\ny\nz\n");
    assert_eq!(old, "");
    assert_eq!(new, "x\r\ny\nz\n");
}
