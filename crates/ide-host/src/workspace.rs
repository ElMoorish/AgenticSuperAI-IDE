use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    Escapes(String),
    NotFound(String),
    Io(String),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Escapes(p) => write!(f, "path escapes workspace: {p}"),
            FsError::NotFound(p) => write!(f, "not found: {p}"),
            FsError::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for FsError {}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Open a workspace rooted at an existing directory.
    pub fn open(root: &Path) -> Result<Workspace, FsError> {
        let canon = canonical(root).map_err(|e| map_io(e, root))?;
        if !canon.is_dir() {
            return Err(FsError::NotFound(root.display().to_string()));
        }
        Ok(Workspace { root: canon })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a relative or absolute path to a canonical path inside the root.
    /// For not-yet-existing paths the nearest existing ancestor is canonicalised
    /// and the remaining (validated) components are appended.
    /// The guarantee is point-in-time (TOCTOU): a junction swapped after
    /// `resolve` returns is not caught.
    pub fn resolve(&self, p: &str) -> Result<PathBuf, FsError> {
        let esc = || FsError::Escapes(p.to_string());
        let raw = Path::new(p);
        // Drive-relative (`C:x`) and rooted-without-drive forms are not
        // absolute on Windows and depend on process state: reject.
        if !raw.is_absolute()
            && (raw.has_root() || matches!(raw.components().next(), Some(Component::Prefix(_))))
        {
            return Err(esc());
        }
        // `:` in a normal component = drive-relative or NTFS alternate stream.
        for c in raw.components() {
            if let Component::Normal(n) = c {
                if n.to_string_lossy().contains(':') || is_reserved_name(n) {
                    return Err(esc());
                }
            }
        }
        let full = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };

        let mut existing = full.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        let canon = loop {
            match canonical(&existing) {
                Ok(c) => break c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Windows may resolve `new/..` lexically; never trust `..`
                    // in a path that does not fully exist.
                    if full
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    {
                        return Err(FsError::Escapes(p.to_string()));
                    }
                    match existing.file_name() {
                        Some(name) => {
                            tail.push(name.to_os_string());
                            existing.pop();
                        }
                        // ".." (or a root with nothing left) in the missing tail
                        None => return Err(FsError::Escapes(p.to_string())),
                    }
                }
                Err(e) => return Err(map_io(e, &full)),
            }
        };

        if !canon.starts_with(&self.root) {
            return Err(FsError::Escapes(p.to_string()));
        }
        let mut out = canon;
        for part in tail.iter().rev() {
            if !safe_tail_part(part) {
                return Err(esc());
            }
            out.push(part);
        }
        if !out.starts_with(&self.root) {
            return Err(esc());
        }
        Ok(out)
    }
}

/// Canonicalise and always strip a `\\?\X:` verbatim prefix (dunce keeps it for
/// long paths), so root and candidates are compared in the same form.
fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    let c = dunce::canonicalize(p)?;
    let s = c.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(c)
}

fn is_reserved_name(part: &std::ffi::OsStr) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let name = part.to_string_lossy();
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let numbered = (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && matches!(stem.as_bytes()[3], b'1'..=b'9');
    numbered || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
}

/// A new path component must be a plain name: no separators, no `:` (drive
/// prefix / ADS), no reserved Windows device names, no trailing dot/space.
fn safe_tail_part(part: &std::ffi::OsStr) -> bool {
    let name = part.to_string_lossy();
    if name.is_empty() || name.contains([':', '\\', '/']) {
        return false;
    }
    if !matches!(
        Path::new(part).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return false;
    }
    if cfg!(windows) && (name.ends_with('.') || name.ends_with(' ') || is_reserved_name(part)) {
        return false;
    }
    true
}

fn map_io(e: std::io::Error, path: &Path) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound(path.display().to_string())
    } else {
        FsError::Io(format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ws() -> (tempfile::TempDir, Workspace) {
        let d = tempfile::tempdir().unwrap();
        let w = Workspace::open(d.path()).unwrap();
        (d, w)
    }

    #[test]
    fn rejects_dotdot() {
        let (d, w) = ws();
        fs::create_dir(d.path().join("sub")).unwrap();
        assert!(matches!(w.resolve("../x"), Err(FsError::Escapes(_))));
        assert!(matches!(w.resolve("sub/../../x"), Err(FsError::Escapes(_))));
        assert!(matches!(w.resolve("new/../x"), Err(FsError::Escapes(_))));
        assert!(matches!(w.resolve(".."), Err(FsError::Escapes(_))));
    }

    #[test]
    fn rejects_absolute_outside() {
        let (_d, w) = ws();
        let other = tempfile::tempdir().unwrap();
        let p = other.path().join("f.txt");
        assert!(matches!(
            w.resolve(p.to_str().unwrap()),
            Err(FsError::Escapes(_))
        ));
    }

    #[test]
    fn rejects_symlink_escape() {
        let (d, w) = ws();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "s").unwrap();
        let link = d.path().join("link");
        #[cfg(windows)]
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        #[cfg(not(windows))]
        let ok = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
        if !ok {
            eprintln!("NOTE: could not create junction/symlink; skipping rejects_symlink_escape");
            return;
        }
        assert!(matches!(
            w.resolve("link/secret.txt"),
            Err(FsError::Escapes(_))
        ));
        assert!(matches!(w.resolve("link/new.txt"), Err(FsError::Escapes(_))));
    }

    #[test]
    fn accepts_nested_and_new_file() {
        let (d, w) = ws();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        fs::write(d.path().join("a/b/f.txt"), "x").unwrap();
        let existing = w.resolve("a/b/f.txt").unwrap();
        assert!(existing.starts_with(w.root()));
        assert!(existing.ends_with("f.txt"));
        let new = w.resolve("a/b/new.txt").unwrap();
        assert!(new.starts_with(w.root()));
        assert_eq!(new.file_name().unwrap(), "new.txt");
        let deep = w.resolve("a/zz/yy/new.txt").unwrap();
        assert!(deep.starts_with(w.root()));
        let abs = w.resolve(existing.to_str().unwrap()).unwrap();
        assert_eq!(abs, existing);
    }

    #[test]
    fn handles_spaces_in_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("a b");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("my file.txt"), "x").unwrap();
        let w = Workspace::open(&root).unwrap();
        let p = w.resolve("my file.txt").unwrap();
        assert!(p.starts_with(w.root()));
        assert!(w.resolve("new dir/n f.txt").is_ok());
    }

    #[test]
    fn open_missing_root_is_not_found() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(
            Workspace::open(&d.path().join("nope")),
            Err(FsError::NotFound(_))
        ));
        let f = d.path().join("f");
        fs::write(&f, "x").unwrap();
        assert!(matches!(Workspace::open(&f), Err(FsError::NotFound(_))));
    }

    fn make_junction(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_drive_prefixed_and_ads_components() {
        let (d, w) = ws();
        fs::write(d.path().join("file.txt"), "x").unwrap();
        for p in ["new/D:x", "new/C:x", "x:ads", "file.txt:ads", "C:x", "D:secret"] {
            assert!(matches!(w.resolve(p), Err(FsError::Escapes(_))), "{p}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_reserved_device_names() {
        let (_d, w) = ws();
        for p in ["new/CON", "new/nul.txt", "COM1", "new/lpt9.log", "aux"] {
            assert!(matches!(w.resolve(p), Err(FsError::Escapes(_))), "{p}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_unc_verbatim_and_rooted() {
        let (_d, w) = ws();
        for p in [r"\\server\share\f", r"\\?\C:\Windows", r"\foo"] {
            assert!(w.resolve(p).is_err(), "{p}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn mixed_separators_and_trailing_dot() {
        let (d, w) = ws();
        fs::create_dir_all(d.path().join("sub/deep")).unwrap();
        let dir = w.resolve("sub/deep").unwrap();
        let m = w.resolve(r"sub/deep\new.txt").unwrap();
        assert!(m.starts_with(&dir));
        // Trailing dot on an existing dir is normalised by Windows: must stay inside.
        match w.resolve("sub./x") {
            Ok(r) => assert!(r.starts_with(w.root())),
            Err(FsError::Escapes(_)) | Err(FsError::NotFound(_)) => {}
            Err(e) => panic!("{e}"),
        }
        // Trailing dot in a new component is rejected.
        assert!(matches!(w.resolve("sub/new."), Err(FsError::Escapes(_))));
    }

    #[test]
    fn root_reached_through_junction() {
        let real = tempfile::tempdir().unwrap();
        fs::write(real.path().join("f.txt"), "x").unwrap();
        let holder = tempfile::tempdir().unwrap();
        let link = holder.path().join("rootlink");
        if !make_junction(&link, real.path()) {
            eprintln!("NOTE: could not create junction; skipping root_reached_through_junction");
            return;
        }
        let w = Workspace::open(&link).unwrap();
        let canon = dunce::canonicalize(real.path()).unwrap();
        assert_eq!(w.root(), canon.as_path());
        let p = w.resolve("f.txt").unwrap();
        assert!(p.starts_with(&canon));
        let via = w.resolve(link.join("f.txt").to_str().unwrap()).unwrap();
        assert!(via.starts_with(&canon));
    }

    #[test]
    fn accepts_path_longer_than_260() {
        let (d, w) = ws();
        let rel = "segment_directory_name/".repeat(12);
        if fs::create_dir_all(d.path().join(&rel)).is_err() {
            eprintln!("NOTE: OS refused long path; skipping accepts_path_longer_than_260");
            return;
        }
        let r = w.resolve(&format!("{rel}new.txt")).unwrap();
        assert!(r.starts_with(w.root()));
        assert!(r.as_os_str().len() > 260);
    }

    #[cfg(windows)]
    #[test]
    fn case_insensitive_root_match() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("f.txt"), "x").unwrap();
        let s = dunce::simplified(d.path()).to_string_lossy().into_owned();
        let mut lower = s.clone();
        lower.replace_range(0..1, &s[0..1].to_lowercase());
        let mut upper = s.clone();
        upper.replace_range(0..1, &s[0..1].to_uppercase());
        let w = Workspace::open(Path::new(&lower)).unwrap();
        let p = w.resolve(&format!(r"{upper}\f.txt")).unwrap();
        assert!(p.starts_with(w.root()));
        let n = w.resolve(&format!(r"{upper}\new.txt")).unwrap();
        assert!(n.starts_with(w.root()));
    }
}
