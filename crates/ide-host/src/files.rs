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
use std::path::{Path, PathBuf};
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

/// Atomically write `text` to `p` (temp file in the same directory, sync, rename).
///
/// With `expected: Some(stamp)` the write is refused with `Conflict` if the
/// file's content hash differs, or the file was deleted. The check is made once
/// up front and again immediately before the rename, which narrows the race
/// but cannot close it: a write landing between that final check and the rename
/// cannot be excluded without OS-level locking. The host does nothing further;
/// the UI's next save will see a `Conflict`. Parent directories created for
/// this write are removed again if the write fails.
pub fn write_atomic(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
) -> Result<FileStamp, WriteError> {
    write_atomic_hooked(ws, p, text, expected, false, None)
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Test hook: `fail_after_temp` fails after the temp file is fully written.
#[doc(hidden)]
pub fn write_atomic_with(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
    fail_after_temp: bool,
) -> Result<FileStamp, WriteError> {
    write_atomic_hooked(ws, p, text, expected, fail_after_temp, None)
}

/// Test hook: `before_rename` runs after the temp file is written and before the
/// final conflict re-check.
#[doc(hidden)]
pub fn write_atomic_hooked(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
    fail_after_temp: bool,
    before_rename: Option<&dyn Fn()>,
) -> Result<FileStamp, WriteError> {
    write_inner(ws, p, text, expected, fail_after_temp, before_rename, false)
}

/// Create `p` only if it does not exist: `Conflict` if the file exists (now, or appears
/// before the final step). Same atomic temp-file write otherwise. On NTFS the final step is
/// a hard link (fails if the target exists), so a concurrent creator cannot be overwritten;
/// filesystems without hard links fall back to an exists-check followed by a rename.
pub fn write_atomic_create_new(ws: &Workspace, p: &str, text: &str) -> Result<FileStamp, WriteError> {
    write_inner(ws, p, text, None, false, None, true)
}

/// Test hook: `before_rename` runs after the temp file is written, before the final step.
#[doc(hidden)]
pub fn write_atomic_create_new_hooked(
    ws: &Workspace,
    p: &str,
    text: &str,
    before_rename: Option<&dyn Fn()>,
) -> Result<FileStamp, WriteError> {
    write_inner(ws, p, text, None, false, before_rename, true)
}

fn write_inner(
    ws: &Workspace,
    p: &str,
    text: &str,
    expected: Option<&FileStamp>,
    fail_after_temp: bool,
    before_rename: Option<&dyn Fn()>,
    create_only: bool,
) -> Result<FileStamp, WriteError> {
    let abs = ws.resolve(p)?;
    let existing = match fs::read(&abs) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(e, &abs).into()),
    };
    if create_only && existing.is_some() {
        return Err(WriteError::Conflict);
    }
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
    // Directories this write will create, deepest first.
    let mut created: Vec<PathBuf> = Vec::new();
    let mut cur = dir.to_path_buf();
    while !cur.exists() {
        created.push(cur.clone());
        if !cur.pop() {
            break;
        }
    }
    let cleanup_dirs = |created: &[PathBuf]| {
        for d in created {
            let _ = fs::remove_dir(d);
        }
    };
    if let Err(e) = fs::create_dir_all(dir) {
        cleanup_dirs(&created);
        return Err(io_err(e, dir).into());
    }
    let name = abs.file_name().unwrap().to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.asa-tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> Result<(), WriteError> {
        let mut f = fs::File::create(&tmp).map_err(|e| io_err(e, &tmp))?;
        f.write_all(&out).map_err(|e| io_err(e, &tmp))?;
        f.flush().map_err(|e| io_err(e, &tmp))?;
        f.sync_all().map_err(|e| io_err(e, &tmp))?;
        drop(f);
        if fail_after_temp {
            return Err(FsError::Io(format!("{}: injected failure", tmp.display())).into());
        }
        if let Some(h) = before_rename {
            h();
        }
        if let Some(exp) = expected {
            match fs::read(&abs) {
                Ok(b) if content_hash(&b) == exp.hash => {}
                Ok(_) => return Err(WriteError::Conflict),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(WriteError::Conflict)
                }
                Err(e) => return Err(io_err(e, &abs).into()),
            }
        }
        if create_only {
            match fs::hard_link(&tmp, &abs) {
                Ok(()) => {
                    let _ = fs::remove_file(&tmp);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(WriteError::Conflict)
                }
                Err(_) => {
                    // no hard links on this filesystem: best-effort check, then rename
                    if abs.exists() {
                        return Err(WriteError::Conflict);
                    }
                    fs::rename(&tmp, &abs).map_err(|e| io_err(e, &abs))?;
                }
            }
        } else {
            fs::rename(&tmp, &abs).map_err(|e| io_err(e, &abs))?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        cleanup_dirs(&created);
        return Err(e);
    }

    let mtime = fs::metadata(&abs)
        .and_then(|m| m.modified())
        .map_err(|e| io_err(e, &abs))?;
    Ok(FileStamp {
        mtime,
        hash: content_hash(&out),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

/// Dominant line ending: CRLF only if the `\r\n` count exceeds the lone `\n`
/// count and is non-zero; otherwise LF (including empty / no newline).
pub fn detect_eol(text: &str) -> Eol {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > 0 && crlf > lf {
        Eol::CrLf
    } else {
        Eol::Lf
    }
}

/// Rewrite every line ending in `text` (CRLF or LF) to `eol`.
pub fn normalise_eol(text: &str, eol: Eol) -> String {
    let lf = text.replace("\r\n", "\n");
    match eol {
        Eol::Lf => lf,
        Eol::CrLf => lf.replace('\n', "\r\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_vectors() {
        assert_eq!(content_hash(b""), 0xcbf29ce484222325);
        assert_eq!(content_hash(b"a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn detect_eol_cases() {
        assert_eq!(detect_eol("a\r\nb\r\n"), Eol::CrLf);
        assert_eq!(detect_eol("a\nb\n"), Eol::Lf);
        assert_eq!(detect_eol("a\r\nb\r\nc\n"), Eol::CrLf);
        assert_eq!(detect_eol("a\r\nb\nc\n"), Eol::Lf);
        assert_eq!(detect_eol(""), Eol::Lf);
        assert_eq!(detect_eol("no newline"), Eol::Lf);
    }

    #[test]
    fn normalise_eol_cases() {
        assert_eq!(normalise_eol("a\nb\r\nc", Eol::CrLf), "a\r\nb\r\nc");
        assert_eq!(normalise_eol("a\nb\r\nc", Eol::Lf), "a\nb\nc");
        assert_eq!(normalise_eol("", Eol::CrLf), "");
    }
}
