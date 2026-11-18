use ide_host::files::{write_atomic_create_new_hooked, write_atomic_create_new, WriteError};
use ide_host::Workspace;
use std::fs;

fn ws() -> (tempfile::TempDir, Workspace) {
    let d = tempfile::tempdir().unwrap();
    let w = Workspace::open(d.path()).unwrap();
    (d, w)
}

#[test]
fn create_new_creates_missing_file_and_parents() {
    let (d, w) = ws();
    let st = write_atomic_create_new(&w, "sub/dir/n.txt", "hello").unwrap();
    assert_eq!(fs::read_to_string(d.path().join("sub/dir/n.txt")).unwrap(), "hello");
    assert_eq!(st.hash, ide_host::files::content_hash(b"hello"));
}

#[test]
fn create_new_refuses_existing_file_and_leaves_it_untouched() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "mine").unwrap();
    let r = write_atomic_create_new(&w, "a.txt", "agent");
    assert!(matches!(r, Err(WriteError::Conflict)));
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "mine");
    let leftovers: Vec<_> = fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(leftovers.len(), 1, "temp file cleaned up: {leftovers:?}");
}

#[test]
fn create_new_detects_file_appearing_just_before_the_rename() {
    let (d, w) = ws();
    let target = d.path().join("race.txt");
    let hook = || fs::write(&target, "appeared").unwrap();
    let r = write_atomic_create_new_hooked(&w, "race.txt", "agent", Some(&hook));
    assert!(matches!(r, Err(WriteError::Conflict)));
    assert_eq!(fs::read_to_string(&target).unwrap(), "appeared");
    assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "temp file cleaned up");
}
