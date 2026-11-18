use ide_host::watcher::*;
use ide_host::Workspace;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

async fn next(rx: &mut UnboundedReceiver<FsBatch>, secs: u64) -> Option<FsBatch> {
    tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await.ok().flatten()
}

fn setup() -> (tempfile::TempDir, Workspace) {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::create_dir_all(d.path().join("target")).unwrap();
    let ws = Workspace::open(d.path()).unwrap();
    (d, ws)
}

#[tokio::test]
async fn watcher_batches_and_ignores_target() {
    let (d, ws) = setup();
    let (_w, mut rx) = Watcher::start(&ws, &[]).unwrap();
    std::fs::write(d.path().join("target/x.o"), "x").unwrap();
    for i in 0..5 {
        std::fs::write(d.path().join("src/a.rs"), format!("v{i}")).unwrap();
    }
    let mut seen: Vec<FsChange> = Vec::new();
    let mut batches_with_a = 0;
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        let wait = if seen.is_empty() { 8 } else { 1 };
        let Some(b) = next(&mut rx, wait).await else { break };
        assert!(!b.overflow);
        if b.changes.iter().any(|c| c.path == "src/a.rs") {
            batches_with_a += 1;
        }
        seen.extend(b.changes);
    }
    assert!(seen.iter().all(|c| !c.path.contains("target")), "{seen:?}");
    let a = seen.iter().find(|c| c.path == "src/a.rs").expect("src/a.rs reported");
    assert_eq!(a.kind, FsKind::Created);
    assert_eq!(batches_with_a, 1, "rapid writes should be one batch: {seen:?}");
}

#[tokio::test]
async fn watcher_custom_ignore_names() {
    let (d, ws) = setup();
    std::fs::create_dir_all(d.path().join("src/gen")).unwrap();
    let (_w, mut rx) = Watcher::start(&ws, &["gen".to_string()]).unwrap();
    std::fs::write(d.path().join("src/gen/o.rs"), "x").unwrap();
    std::fs::write(d.path().join("src/keep.rs"), "x").unwrap();
    let b = next(&mut rx, 8).await.unwrap();
    assert!(b.changes.iter().any(|c| c.path == "src/keep.rs"));
    assert!(b.changes.iter().all(|c| !c.path.contains("gen")), "{b:?}");
}

#[tokio::test]
async fn watcher_created_then_removed_cancels_out() {
    let (d, ws) = setup();
    let (_w, mut rx) = Watcher::start(&ws, &[]).unwrap();
    std::fs::write(d.path().join("src/tmp.txt"), "x").unwrap();
    std::fs::remove_file(d.path().join("src/tmp.txt")).unwrap();
    std::fs::write(d.path().join("src/marker.txt"), "x").unwrap();
    let mut all = Vec::new();
    while let Some(b) = next(&mut rx, 3).await {
        all.extend(b.changes);
        if all.iter().any(|c| c.path == "src/marker.txt") {
            break;
        }
    }
    assert!(all.iter().any(|c| c.path == "src/marker.txt"), "{all:?}");
    assert!(all.iter().all(|c| c.path != "src/tmp.txt"), "{all:?}");
}

#[tokio::test]
async fn watcher_overflow_flag_on_mass_change() {
    let (d, ws) = setup();
    let (_w, mut rx) = Watcher::start(&ws, &[]).unwrap();
    let dir = d.path().join("src");
    let t0 = Instant::now();
    let hs: Vec<_> = (0..8)
        .map(|t| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                for i in (t * 750)..((t + 1) * 750) {
                    std::fs::File::create(dir.join(format!("f{i}.txt"))).unwrap();
                }
            })
        })
        .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
    eprintln!("created 6000 files in {:?}", t0.elapsed());
    let mut overflow = false;
    let end = Instant::now() + Duration::from_secs(60);
    while Instant::now() < end && !overflow {
        match next(&mut rx, 10).await {
            Some(b) => {
                eprintln!("batch {} overflow {}", b.changes.len(), b.overflow);
                assert!(b.changes.len() <= 5000);
                overflow = b.overflow;
            }
            None => break,
        }
    }
    assert!(overflow, "expected an overflow batch");
}

#[tokio::test]
async fn watcher_drop_ends_stream() {
    let (_d, ws) = setup();
    let (w, mut rx) = Watcher::start(&ws, &[]).unwrap();
    drop(w);
    assert!(next(&mut rx, 5).await.is_none());
}

#[test]
fn watcher_events_outside_root_are_dropped() {
    use notify::event::{CreateKind, ModifyKind, RenameMode};
    use notify::{Event, EventKind};
    let root = Path::new("/ws");
    let ign = default_ignore(&[]);
    let ev = Event::new(EventKind::Create(CreateKind::File))
        .add_path("/ws/src/a.rs".into())
        .add_path("/other/b.rs".into())
        .add_path("/ws/target/c.o".into())
        .add_path("/ws/node_modules/x/y.js".into());
    let out = map_event(root, &ign, &ev);
    assert_eq!(out, vec![("src/a.rs".to_string(), FsKind::Created)]);
    let ev = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
        .add_path("/ws/src/old.rs".into())
        .add_path("/ws/src/new.rs".into());
    assert_eq!(
        map_event(root, &ign, &ev),
        vec![("src/old.rs".to_string(), FsKind::Removed), ("src/new.rs".to_string(), FsKind::Created)]
    );
    let ev = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path("/ws/a".into());
    assert_eq!(map_event(root, &ign, &ev), vec![("a".to_string(), FsKind::Removed)]);
    let ev = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To))).add_path("/ws/a".into());
    assert_eq!(map_event(root, &ign, &ev), vec![("a".to_string(), FsKind::Created)]);
}
