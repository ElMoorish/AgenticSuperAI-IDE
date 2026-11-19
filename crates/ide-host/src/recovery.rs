//! Crash recovery for unsaved editor buffers.
//!
//! The UI snapshots every dirty buffer here every few seconds. After a crash the snapshots are
//! offered back; a clean save or an explicit discard clears them. Snapshots live in the app data
//! directory, never in the project, and are written atomically (temp file, then rename), so a
//! crash while saving leaves the previous snapshot intact.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A single snapshot is capped so a huge buffer cannot fill the disk every 5 seconds.
pub const MAX_BUFFER_BYTES: usize = 5 * 1024 * 1024;
/// At most this many snapshots are kept per workspace.
pub const MAX_BUFFERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredBuffer {
    pub rel: String,
    pub text: String,
    pub saved_at_ms: u64,
}

#[derive(Debug)]
pub enum RecoveryError {
    TooLarge,
    TooMany,
    Io(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoveryError::TooLarge => write!(f, "buffer is too large to keep a recovery copy"),
            RecoveryError::TooMany => write!(f, "too many recovery copies"),
            RecoveryError::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for RecoveryError {}

fn io<E: std::fmt::Display>(e: E) -> RecoveryError {
    RecoveryError::Io(e.to_string())
}

/// FNV-1a: stable across runs and Rust versions (unlike `DefaultHasher`).
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub struct RecoveryStore {
    dir: PathBuf,
}

impl RecoveryStore {
    /// `base` is `<app data>/recovery`; each workspace gets its own sub folder.
    pub fn new(base: &Path, workspace_root: &str) -> RecoveryStore {
        RecoveryStore { dir: base.join(format!("{:016x}", fnv(workspace_root))) }
    }

    fn file_for(&self, rel: &str) -> PathBuf {
        // Case-folded like the editor's path identity, so Foo.txt and foo.txt share one snapshot.
        self.dir.join(format!("{:016x}.json", fnv(&rel.to_lowercase())))
    }

    pub fn save_buffer(&self, rel: &str, text: &str) -> Result<(), RecoveryError> {
        if text.len() > MAX_BUFFER_BYTES {
            return Err(RecoveryError::TooLarge);
        }
        fs::create_dir_all(&self.dir).map_err(io)?;
        let path = self.file_for(rel);
        if !path.exists() && self.count() >= MAX_BUFFERS {
            return Err(RecoveryError::TooMany);
        }
        let rec = RecoveredBuffer { rel: rel.to_string(), text: text.to_string(), saved_at_ms: now_ms() };
        let body = serde_json::to_vec(&rec).map_err(io)?;
        let tmp = self.dir.join(format!(".tmp-{:016x}-{}", fnv(rel), std::process::id()));
        {
            let mut f = fs::File::create(&tmp).map_err(io)?;
            f.write_all(&body).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(io(e));
        }
        Ok(())
    }

    fn count(&self) -> usize {
        fs::read_dir(&self.dir)
            .map(|d| d.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "json")).count())
            .unwrap_or(0)
    }

    /// Every readable snapshot, oldest first. Corrupt or foreign files are skipped, never fatal.
    pub fn list(&self) -> Vec<RecoveredBuffer> {
        let mut out: Vec<RecoveredBuffer> = fs::read_dir(&self.dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| fs::read(e.path()).ok())
                    .filter_map(|b| serde_json::from_slice::<RecoveredBuffer>(&b).ok())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.saved_at_ms.cmp(&b.saved_at_ms).then_with(|| a.rel.cmp(&b.rel)));
        out
    }

    pub fn clear(&self, rel: &str) {
        let _ = fs::remove_file(self.file_for(rel));
    }

    pub fn clear_all(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_roundtrip_after_simulated_crash() {
        let base = tempfile::tempdir().unwrap();
        {
            let s = RecoveryStore::new(base.path(), "C:/proj");
            s.save_buffer("src/main.cpp", "int main() {\r\n  // unsaved é\r\n}").unwrap();
            s.save_buffer("src/main.cpp", "int main() { /* newer */ }").unwrap(); // overwrites
            s.save_buffer("notes.txt", "hello").unwrap();
        } // the process "crashes": the store value is gone, only the files remain
        let s = RecoveryStore::new(base.path(), "C:/proj");
        let got = s.list();
        assert_eq!(got.len(), 2);
        let m = got.iter().find(|b| b.rel == "src/main.cpp").unwrap();
        assert_eq!(m.text, "int main() { /* newer */ }");
        s.clear("src/main.cpp");
        assert_eq!(s.list().len(), 1);
        // another workspace sees nothing
        assert!(RecoveryStore::new(base.path(), "C:/other").list().is_empty());
        s.clear_all();
        assert!(s.list().is_empty());
    }

    #[test]
    fn exact_text_is_preserved_and_case_variants_share_a_snapshot() {
        let base = tempfile::tempdir().unwrap();
        let s = RecoveryStore::new(base.path(), "w");
        let text = "a\r\nb\n\u{feff}tab\t \u{0} end";
        s.save_buffer("Dir/File.TXT", text).unwrap();
        s.save_buffer("dir/file.txt", "second").unwrap();
        let l = s.list();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].text, "second");
        s.save_buffer("x", text).unwrap();
        assert_eq!(s.list().iter().find(|b| b.rel == "x").unwrap().text, text);
    }

    #[test]
    fn corrupt_files_are_ignored_and_limits_hold() {
        let base = tempfile::tempdir().unwrap();
        let s = RecoveryStore::new(base.path(), "w");
        s.save_buffer("ok", "fine").unwrap();
        let dir = fs::read_dir(base.path()).unwrap().next().unwrap().unwrap().path();
        fs::write(dir.join("garbage.json"), b"{not json").unwrap();
        assert_eq!(s.list().len(), 1);
        assert!(matches!(s.save_buffer("big", &"x".repeat(MAX_BUFFER_BYTES + 1)), Err(RecoveryError::TooLarge)));
    }
}
