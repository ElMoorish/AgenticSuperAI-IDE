use std::fmt;
use std::path::{Path, PathBuf};

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
        let canon = dunce::canonicalize(root).map_err(|e| map_io(e, root))?;
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
    /// and the remaining components are appended (`..` in that tail is rejected).
    pub fn resolve(&self, p: &str) -> Result<PathBuf, FsError> {
        let raw = Path::new(p);
        let full = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.root.join(raw)
        };

        let mut existing = full.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        let canon = loop {
            match std::fs::canonicalize(&existing) {
                Ok(c) => break dunce::simplified(&c).to_path_buf(),
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
            out.push(part);
        }
        Ok(out)
    }
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
