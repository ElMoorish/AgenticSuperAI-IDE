//! Debounced workspace file watcher.
//!
//! Ignore rules: a path is dropped when ANY of its workspace-relative
//! components equals (ASCII case-insensitively) one of the default names
//! (`target`, `node_modules`, `.git`, `build`) or a caller-supplied name.
//! Names only; no globs in v1.

use crate::Workspace;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const QUIET: Duration = Duration::from_millis(100);
const BURST_QUIET: Duration = Duration::from_millis(1500);
/// Upper bound on how long a continuous event stream may delay a flush.
const MAX_WINDOW: Duration = Duration::from_secs(5);
const MAX_PATHS: usize = 5000;
const DEFAULT_IGNORE: [&str; 4] = ["target", "node_modules", ".git", "build"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsChange {
    /// Workspace-relative, forward slashes.
    pub path: String,
    pub kind: FsKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FsBatch {
    pub changes: Vec<FsChange>,
    /// More than 5000 paths changed (or the OS dropped events): do a full refresh.
    pub overflow: bool,
}

#[derive(Debug)]
pub enum WatchError {
    Notify(String),
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchError::Notify(m) => write!(f, "watch error: {m}"),
        }
    }
}
impl std::error::Error for WatchError {}

/// Dropping the `Watcher` stops watching and ends the batch stream.
pub struct Watcher {
    _inner: RecommendedWatcher,
}

enum Raw {
    Changes(Vec<(String, FsKind)>),
    Rescan,
}

#[doc(hidden)]
pub fn default_ignore(extra: &[String]) -> Vec<String> {
    DEFAULT_IGNORE.iter().map(|s| s.to_string()).chain(extra.iter().cloned()).collect()
}

fn rel_path(root: &Path, ignore: &[String], p: &Path) -> Option<String> {
    let rel = p.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => {
                let n = n.to_string_lossy();
                if ignore.iter().any(|i| i.eq_ignore_ascii_case(&n)) {
                    return None;
                }
                parts.push(n.into_owned());
            }
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Pure mapping of one notify event to workspace-relative changes.
/// Paths outside `root` or matching `ignore` are dropped.
#[doc(hidden)]
pub fn map_event(root: &Path, ignore: &[String], ev: &Event) -> Vec<(String, FsKind)> {
    let rel = |p: &Path| rel_path(root, ignore, p);
    let mut out = Vec::new();
    let push_each = |kind: FsKind, out: &mut Vec<(String, FsKind)>| {
        for p in &ev.paths {
            if let Some(r) = rel(p) {
                out.push((r, kind));
            }
        }
    };
    match &ev.kind {
        EventKind::Create(_) => push_each(FsKind::Created, &mut out),
        EventKind::Remove(_) => push_each(FsKind::Removed, &mut out),
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::From => push_each(FsKind::Removed, &mut out),
            RenameMode::To => push_each(FsKind::Created, &mut out),
            RenameMode::Both if ev.paths.len() == 2 => {
                if let Some(r) = rel(&ev.paths[0]) {
                    out.push((r, FsKind::Removed));
                }
                if let Some(r) = rel(&ev.paths[1]) {
                    out.push((r, FsKind::Created));
                }
            }
            // Ambiguous (note: one exists() stat on the callback thread, accepted): decide by whether the path still exists.
            _ => {
                for p in &ev.paths {
                    if let Some(r) = rel(p) {
                        let kind = if p.exists() { FsKind::Created } else { FsKind::Removed };
                        out.push((r, kind));
                    }
                }
            }
        },
        EventKind::Modify(_) => push_each(FsKind::Modified, &mut out),
        EventKind::Any => push_each(FsKind::Modified, &mut out),
        EventKind::Access(_) | EventKind::Other => {}
    }
    out
}

/// Dedupe edge cases: C+M stays Created; C+R cancels out entirely (the UI
/// never saw the file); R+C or R+M becomes Modified (atomic-replace pattern:
/// delete then recreate), so a replaced file is refreshed rather than dropped.
fn merge(prev: FsKind, new: FsKind) -> Option<FsKind> {
    use FsKind::*;
    match (prev, new) {
        (Created, Created | Modified) => Some(Created),
        (Created, Removed) => None,
        (Modified, Created | Modified) => Some(Modified),
        (Modified, Removed) => Some(Removed),
        (Removed, Created | Modified) => Some(Modified),
        (Removed, Removed) => Some(Removed),
    }
}

#[derive(Default)]
struct Window {
    map: HashMap<String, (usize, FsKind)>,
    seq: usize,
    overflow: bool,
}

impl Window {
    fn add(&mut self, path: String, kind: FsKind) {
        match self.map.get(&path).copied() {
            Some((order, prev)) => match merge(prev, kind) {
                Some(k) => {
                    self.map.insert(path, (order, k));
                }
                None => {
                    self.map.remove(&path);
                }
            },
            None => {
                if self.map.len() >= MAX_PATHS {
                    self.overflow = true;
                    return;
                }
                self.map.insert(path, (self.seq, kind));
                self.seq += 1;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty() && !self.overflow
    }

    fn into_batch(self) -> FsBatch {
        // add() never lets the map exceed MAX_PATHS, so no truncation is needed.
        let mut v: Vec<_> = self.map.into_iter().collect();
        v.sort_by_key(|(_, (o, _))| *o);
        FsBatch {
            changes: v.into_iter().map(|(path, (_, kind))| FsChange { path, kind }).collect(),
            overflow: self.overflow,
        }
    }
}

fn debounce_loop(rx: mpsc::Receiver<Raw>, tx: UnboundedSender<FsBatch>) {
    loop {
        let first = match rx.recv() {
            Ok(r) => r,
            Err(_) => return,
        };
        let mut win = Window::default();
        let apply = |w: &mut Window, r: Raw| match r {
            Raw::Changes(cs) => cs.into_iter().for_each(|(p, k)| w.add(p, k)),
            Raw::Rescan => w.overflow = true,
        };
        apply(&mut win, first);
        let started = Instant::now();
        let mut disconnected = false;
        loop {
            if win.overflow && win.map.len() >= MAX_PATHS || started.elapsed() >= MAX_WINDOW {
                break;
            }
            // During a large burst (e.g. a checkout) OS delivery can stall
            // for longer than 100 ms mid-stream; wait longer so it stays one batch.
            let quiet = if win.map.len() > 500 { BURST_QUIET } else { QUIET };
            match rx.recv_timeout(quiet) {
                Ok(r) => apply(&mut win, r),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if !win.is_empty() && tx.send(win.into_batch()).is_err() {
            return;
        }
        if disconnected {
            return;
        }
    }
}

impl Watcher {
    pub fn start(
        ws: &Workspace,
        ignore: &[String],
    ) -> Result<(Watcher, UnboundedReceiver<FsBatch>), WatchError> {
        let root = ws.root().to_path_buf();
        let ign = default_ignore(ignore);
        let (raw_tx, raw_rx) = mpsc::channel::<Raw>();
        let (tx, rx) = unbounded_channel();
        let cb_root = root.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
            // Never block or panic here: std mpsc send is unbounded.
            let msg = match res {
                Ok(ev) if ev.need_rescan() => Raw::Rescan,
                Ok(ev) => {
                    let c = map_event(&cb_root, &ign, &ev);
                    if c.is_empty() {
                        return;
                    }
                    Raw::Changes(c)
                }
                Err(_) => Raw::Rescan,
            };
            let _ = raw_tx.send(msg);
        })
        .map_err(|e| WatchError::Notify(e.to_string()))?;
        inner
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| WatchError::Notify(e.to_string()))?;
        std::thread::Builder::new()
            .name("fs-debounce".into())
            .spawn(move || debounce_loop(raw_rx, tx))
            .map_err(|e| WatchError::Notify(e.to_string()))?;
        Ok((Watcher { _inner: inner }, rx))
    }
}
