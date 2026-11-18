//! Editor-side file handling: stamped reads and atomic, conflict-guarded writes.
//!
//! Stamp equality: a `FileStamp` is `(mtime, hash)`. mtime can be coarse (or be
//! touched without a content change), so the content hash is the tiebreaker:
//! two stamps match when their hashes are equal, regardless of mtime. A
//! different hash is a conflict. The hash is 64-bit FNV-1a over the raw file
//! bytes (BOM included): fixed-key, stable across runs and platforms, no
//! dependency. It is a change detector, not a security primitive.

use crate::workspace::{FsError, Workspace};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub mtime: SystemTime,
    pub hash: u64,
}

impl FileStamp {
    /// Same content (hash equal); mtime alone never makes stamps differ.
    pub fn matches(&self, other: &FileStamp) -> bool {
        self.hash == other.hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    Conflict,
    Fs(FsError),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Conflict => write!(f, "file changed on disk since it was loaded"),
            WriteError::Fs(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<FsError> for WriteError {
    fn from(e: FsError) -> Self {
        WriteError::Fs(e)
    }
}

/// 64-bit FNV-1a.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn io_err(e: std::io::Error, path: &Path) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound(path.display().to_string())
    } else {
        FsError::Io(format!("{}: {e}", path.display()))
    }
}

/// Read the file at `p`, decode UTF-8, strip a leading BOM. Line endings are
/// returned exactly as stored.
pub fn read_stamped(ws: &Workspace, p: &str) -> Result<(String, FileStamp), FsError> {
    let abs = ws.resolve(p)?;
    let bytes = fs::read(&abs).map_err(|e| io_err(e, &abs))?;
    let mtime = fs::metadata(&abs)
        .and_then(|m| m.modified())
        .map_err(|e| io_err(e, &abs))?;
    let hash = content_hash(&bytes);
    let body = bytes.strip_prefix(BOM).unwrap_or(&bytes);
    let text = std::str::from_utf8(body)
        .map_err(|_| FsError::Io(format!("{}: file is not valid UTF-8", abs.display())))?;
    Ok((text.to_string(), FileStamp { mtime, hash }))
}

pub fn write_atomic(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
) -> Result<FileStamp, WriteError> {
    write_atomic_with(ws, p, text, expected, false)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// As `write_atomic`; `fail_after_temp` is a test hook that fails after the temp
/// file has been fully written, to prove the original survives and the temp is
/// cleaned up.
#[doc(hidden)]
pub fn write_atomic_with(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
    fail_after_temp: bool,
) -> Result<FileStamp, WriteError> {
    let abs = ws.resolve(p)?;
    let existing = match fs::read(&abs) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(e, &abs).into()),
    };
    match (&existing, expected) {
        (Some(b), Some(exp)) if content_hash(b) != exp.hash => return Err(WriteError::Conflict),
        (None, Some(_)) => return Err(WriteError::Conflict),
        _ => {}
    }

    let had_bom = existing.as_deref().is_some_and(|b| b.starts_with(BOM));
    let mut out: Vec<u8> = Vec::with_capacity(text.len() + 3);
    if had_bom && !text.starts_with('\u{FEFF}') {
        out.extend_from_slice(BOM);
    }
    out.extend_from_slice(text.as_bytes());

    let dir = abs
        .parent()
        .ok_or_else(|| FsError::Io(format!("{}: no parent directory", abs.display())))?;
    fs::create_dir_all(dir).map_err(|e| io_err(e, dir))?;
    let name = abs.file_name().unwrap().to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.asa-tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> Result<(), FsError> {
        let mut f = fs::File::create(&tmp).map_err(|e| io_err(e, &tmp))?;
        f.write_all(&out).map_err(|e| io_err(e, &tmp))?;
        f.flush().map_err(|e| io_err(e, &tmp))?;
        f.sync_all().map_err(|e| io_err(e, &tmp))?;
        drop(f);
        if fail_after_temp {
            return Err(FsError::Io(format!("{}: injected failure", tmp.display())));
        }
        fs::rename(&tmp, &abs).map_err(|e| io_err(e, &abs))
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(WriteError::Fs(e));
    }

    let mtime = fs::metadata(&abs)
        .and_then(|m| m.modified())
        .map_err(|e| io_err(e, &abs))?;
    Ok(FileStamp {
        mtime,
        hash: content_hash(&out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_vectors() {
        assert_eq!(content_hash(b""), 0xcbf29ce484222325);
        assert_eq!(content_hash(b"a"), 0xaf63dc4c8601ec8c);
    }
}
