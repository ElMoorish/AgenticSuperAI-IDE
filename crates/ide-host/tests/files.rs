use ide_host::editor::{BufferView, EditorHandler, HandlerOutcome};
use ide_host::files::{read_stamped, write_atomic, write_atomic_with, FileStamp, WriteError};
use ide_host::{FsError, Workspace};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

#[derive(Default)]
struct Bufs(Mutex<HashMap<PathBuf, String>>);
impl BufferView for Bufs {
    fn open_buffer(&self, abs: &Path) -> Option<String> {
        self.0.lock().unwrap().get(abs).cloned()
    }
}

fn ws() -> (tempfile::TempDir, Workspace) {
    let d = tempfile::tempdir().unwrap();
    let w = Workspace::open(d.path()).unwrap();
    (d, w)
}

fn handler(w: &Workspace) -> EditorHandler<Bufs> {
    EditorHandler::new(w.clone(), Bufs::default())
}

#[test]
fn read_returns_open_buffer_when_dirty() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "disk").unwrap();
    let h = handler(&w);
    let abs = w.resolve("a.txt").unwrap();
    h.buffers().0.lock().unwrap().insert(abs, "dirty".into());
    let out = h.handle(json!(1), "fs/read_text_file", &json!({"path":"a.txt"}));
    match out {
        HandlerOutcome::Reply(v) => assert_eq!(v["content"], "dirty"),
        o => panic!("{o:?}"),
    }
    // no buffer -> disk
    fs::write(d.path().join("b.txt"), "ondisk").unwrap();
    match h.handle(json!(2), "fs/read_text_file", &json!({"path":"b.txt"})) {
        HandlerOutcome::Reply(v) => assert_eq!(v["content"], "ondisk"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn write_refused_on_conflict() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "one").unwrap();
    let (_, stamp) = read_stamped(&w, "a.txt").unwrap();
    fs::write(d.path().join("a.txt"), "two").unwrap();
    let r = write_atomic(&w, "a.txt", "three", Some(&stamp));
    assert!(matches!(r, Err(WriteError::Conflict)));
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "two");
    // without a guard it overwrites
    write_atomic(&w, "a.txt", "three", None).unwrap();
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "three");
}

#[test]
fn write_is_atomic_no_partial_file() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "original").unwrap();
    let r = write_atomic_with(&w, "a.txt", "new content", None, true);
    assert!(matches!(r, Err(WriteError::Fs(_))));
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "original");
    let names: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["a.txt".to_string()], "temp file left behind");
}

#[test]
fn crlf_preserved_on_roundtrip() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), b"a\r\nb\r\nc\n").unwrap();
    let (t, s) = read_stamped(&w, "a.txt").unwrap();
    assert_eq!(t, "a\r\nb\r\nc\n");
    write_atomic(&w, "a.txt", &t, Some(&s)).unwrap();
    assert_eq!(fs::read(d.path().join("a.txt")).unwrap(), b"a\r\nb\r\nc\n");
}

#[test]
fn bom_preserved() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), b"\xEF\xBB\xBFhello").unwrap();
    let (t, s) = read_stamped(&w, "a.txt").unwrap();
    assert_eq!(t, "hello");
    write_atomic(&w, "a.txt", "hello world", Some(&s)).unwrap();
    assert_eq!(
        fs::read(d.path().join("a.txt")).unwrap(),
        b"\xEF\xBB\xBFhello world"
    );
    // a file without BOM stays without
    fs::write(d.path().join("b.txt"), b"x").unwrap();
    write_atomic(&w, "b.txt", "y", None).unwrap();
    assert_eq!(fs::read(d.path().join("b.txt")).unwrap(), b"y");
}

#[test]
fn unicode_and_spaced_path_roundtrip() {
    let (d, w) = ws();
    fs::create_dir_all(d.path().join("my dir/dossier é")).unwrap();
    let p = "my dir/dossier é/fichier ünï 日本.txt";
    write_atomic(&w, p, "héllo 日本\n", None).unwrap();
    let (t, s) = read_stamped(&w, p).unwrap();
    assert_eq!(t, "héllo 日本\n");
    write_atomic(&w, p, "again ✓", Some(&s)).unwrap();
    assert_eq!(read_stamped(&w, p).unwrap().0, "again ✓");
}

#[test]
fn agent_write_becomes_pending_edit_not_disk_write() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "old").unwrap();
    let h = handler(&w);
    let out = h.handle(
        json!("req-7"),
        "fs/write_text_file",
        &json!({"path":"a.txt","content":"new"}),
    );
    match out {
        HandlerOutcome::NeedsApproval(e) => {
            assert_eq!(e.path, "a.txt");
            assert_eq!(e.old, "old");
            assert_eq!(e.new, "new");
            assert_eq!(e.agent_id, json!("req-7"));
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "old");
    // new file: old is empty, nothing created
    match h.handle(
        json!(8),
        "fs/write_text_file",
        &json!({"path":"sub/n.txt","content":"z"}),
    ) {
        HandlerOutcome::NeedsApproval(e) => {
            assert_eq!(e.old, "");
            assert_eq!(e.path, "sub/n.txt");
        }
        o => panic!("{o:?}"),
    }
    assert!(!d.path().join("sub").exists());
}

#[test]
fn invalid_utf8_is_an_error() {
    let (d, w) = ws();
    fs::write(d.path().join("a.bin"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    let e = read_stamped(&w, "a.bin").unwrap_err();
    assert!(e.to_string().contains("file is not valid UTF-8"), "{e}");
    let h = handler(&w);
    match h.handle(json!(1), "fs/read_text_file", &json!({"path":"a.bin"})) {
        HandlerOutcome::Error(v) => {
            assert!(v["message"].as_str().unwrap().contains("not valid UTF-8"))
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn path_escape_returns_error_not_panic() {
    let (_d, w) = ws();
    assert!(matches!(read_stamped(&w, "../x"), Err(FsError::Escapes(_))));
    assert!(matches!(
        write_atomic(&w, "../x", "a", None),
        Err(WriteError::Fs(FsError::Escapes(_)))
    ));
    let h = handler(&w);
    for (m, p) in [
        ("fs/read_text_file", json!({"path":"../x"})),
        ("fs/write_text_file", json!({"path":"../x","content":"a"})),
    ] {
        match h.handle(json!(1), m, &p) {
            HandlerOutcome::Error(v) => assert!(v["code"].is_i64()),
            o => panic!("{o:?}"),
        }
    }
}

#[test]
fn missing_file_read_returns_error_with_code() {
    let (_d, w) = ws();
    let h = handler(&w);
    match h.handle(json!(1), "fs/read_text_file", &json!({"path":"nope.txt"})) {
        HandlerOutcome::Error(v) => {
            assert_eq!(v["code"], -32002);
            assert!(v["message"].as_str().unwrap().contains("not found"));
        }
        o => panic!("{o:?}"),
    }
    match h.handle(json!(2), "terminal/create", &json!({})) {
        HandlerOutcome::Error(v) => assert_eq!(v["code"], -32601),
        o => panic!("{o:?}"),
    }
}

#[test]
fn deleted_since_load_is_conflict() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "x").unwrap();
    let (_, s) = read_stamped(&w, "a.txt").unwrap();
    fs::remove_file(d.path().join("a.txt")).unwrap();
    assert!(matches!(
        write_atomic(&w, "a.txt", "y", Some(&s)),
        Err(WriteError::Conflict)
    ));
    assert!(!d.path().join("a.txt").exists());
}

#[test]
fn same_content_different_mtime_is_not_conflict() {
    let (d, w) = ws();
    fs::write(d.path().join("a.txt"), "same").unwrap();
    let (_, mut s) = read_stamped(&w, "a.txt").unwrap();
    s.mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let s2: FileStamp = write_atomic(&w, "a.txt", "next", Some(&s)).unwrap();
    assert_eq!(fs::read_to_string(d.path().join("a.txt")).unwrap(), "next");
    assert_ne!(s2.hash, s.hash);
}

#[test]
fn permission_request_needs_permission() {
    let (_d, w) = ws();
    let h = handler(&w);
    let p = json!({"toolCall":{"title":"Run ls","content":"ls -la"}});
    match h.handle(json!(3), "session/request_permission", &p) {
        HandlerOutcome::NeedsPermission(c) => {
            assert_eq!(c.title, "Run ls");
            assert_eq!(c.command, "ls -la");
            assert_eq!(c.agent_id, Value::from(3));
        }
        o => panic!("{o:?}"),
    }
}
