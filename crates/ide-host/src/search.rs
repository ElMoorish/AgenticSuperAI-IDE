//! Read-only project navigation shared by the agent (list_dir / find_files / search_text) and the
//! UI's quick-open: list a folder, find files by name, search file contents. Everything is bounded
//! (entries, files visited, bytes read, results) so a huge or hostile tree cannot stall the host.

use crate::watcher::default_ignore;
use crate::workspace::{FsError, Workspace};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;

/// Most files a walk will visit.
pub const MAX_WALK: usize = 20_000;
/// Largest file searched for text.
const MAX_GREP_BYTES: u64 = 1024 * 1024;
const MAX_LINE_CHARS: usize = 200;

fn ignored(name: &str, ignore: &[String]) -> bool {
    ignore.iter().any(|i| i.eq_ignore_ascii_case(name))
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every file under the workspace (breadth first, so shallow files come first), skipping ignored
/// names and never following symlinks or junctions. Stops at `cap` files.
pub fn walk_files(ws: &Workspace, extra_ignore: &[String], cap: usize) -> Vec<String> {
    let ignore = default_ignore(extra_ignore);
    let root = ws.root().to_path_buf();
    let mut out = Vec::new();
    let mut queue = VecDeque::from([root.clone()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if ignored(&name, &ignore) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                queue.push_back(e.path());
            } else if ft.is_file() {
                out.push(rel_of(&root, &e.path()));
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

/// Files whose name or path matches `query` (case-insensitive), best matches first:
/// name starts with it, name contains it, path contains it, then the letters in order in the name.
pub fn find_files(ws: &Workspace, extra_ignore: &[String], query: &str, limit: usize) -> Vec<String> {
    let files = walk_files(ws, extra_ignore, MAX_WALK);
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return files.into_iter().take(limit).collect();
    }
    let mut scored: Vec<(u8, usize, String)> = files
        .into_iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            let base = lower.rsplit('/').next().unwrap_or(&lower).to_string();
            let rank = if base.starts_with(&q) {
                0
            } else if base.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else if is_subsequence(&q, &base) {
                3
            } else {
                return None;
            };
            Some((rank, f.len(), f))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(_, _, f)| f).collect()
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// `path:line: text` for each line containing `needle` (case-insensitive), at most `limit` lines.
pub fn search_text(ws: &Workspace, extra_ignore: &[String], needle: &str, limit: usize) -> Vec<String> {
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for rel in walk_files(ws, extra_ignore, MAX_WALK) {
        let abs = ws.root().join(&rel);
        match fs::metadata(&abs) {
            Ok(m) if m.len() <= MAX_GREP_BYTES => {}
            _ => continue,
        }
        let Ok(bytes) = fs::read(&abs) else { continue };
        if bytes.contains(&0) {
            continue; // binary
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&n) {
                let shown: String = line.trim().chars().take(MAX_LINE_CHARS).collect();
                out.push(format!("{rel}:{}: {shown}", i + 1));
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// One line per entry, folders first with a trailing `/`, files with their size. `rel` "" or "."
/// is the project root. Truncation is stated, never silent.
pub fn list_dir_text(ws: &Workspace, extra_ignore: &[String], rel: &str, cap: usize) -> Result<String, FsError> {
    let rel = if rel == "." { "" } else { rel };
    let abs = ws.resolve(rel)?;
    let ignore = default_ignore(extra_ignore);
    let rd = fs::read_dir(&abs).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound(rel.to_string())
        } else {
            FsError::Io(format!("cannot list {rel}: {e}"))
        }
    })?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if ignored(&name, &ignore) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            dirs.push(format!("{name}/"));
        } else {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            files.push(format!("{name} ({size} bytes)"));
        }
    }
    dirs.sort_by_key(|s| s.to_lowercase());
    files.sort_by_key(|s| s.to_lowercase());
    let total = dirs.len() + files.len();
    let mut lines: Vec<String> = dirs.into_iter().chain(files).collect();
    lines.truncate(cap);
    let mut out = if lines.is_empty() { "(empty)".to_string() } else { lines.join("\n") };
    if total > cap {
        out.push_str(&format!("\n... and {} more entries", total - cap));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Workspace) {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        fs::create_dir_all(r.join("src/ui")).unwrap();
        fs::create_dir_all(r.join("node_modules/x")).unwrap();
        fs::create_dir_all(r.join(".git")).unwrap();
        fs::write(r.join("README.md"), "# Hello\nsecond line\n").unwrap();
        fs::write(r.join("main.lua"), "print('hi')\nlocal Needle = 1\n").unwrap();
        fs::write(r.join("src/ui/panel.lua"), "-- needle again\n").unwrap();
        fs::write(r.join("src/util.lua"), "return {}\n").unwrap();
        fs::write(r.join("node_modules/x/skip.lua"), "needle\n").unwrap();
        fs::write(r.join(".git/config"), "needle\n").unwrap();
        fs::write(r.join("bin.dat"), b"needle\0binary").unwrap();
        let ws = Workspace::open(r).unwrap();
        (d, ws)
    }

    #[test]
    fn list_dir_shows_folders_first_and_hides_ignored() {
        let (_d, ws) = project();
        let t = list_dir_text(&ws, &[], "", 100).unwrap();
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "src/");
        assert!(lines.iter().any(|l| l.starts_with("README.md (")));
        assert!(!t.contains("node_modules") && !t.contains(".git"), "{t}");
        assert_eq!(list_dir_text(&ws, &[], ".", 100).unwrap(), t, "'.' is the root");
        assert!(list_dir_text(&ws, &[], "src/ui", 100).unwrap().contains("panel.lua"));
        assert!(matches!(list_dir_text(&ws, &[], "nope", 10), Err(FsError::NotFound(_))));
        assert!(list_dir_text(&ws, &[], "../..", 10).is_err(), "cannot leave the workspace");
        let capped = list_dir_text(&ws, &[], "", 2).unwrap();
        assert!(capped.contains("more entries"), "{capped}");
    }

    #[test]
    fn find_files_ranks_name_matches_first() {
        let (_d, ws) = project();
        assert_eq!(find_files(&ws, &[], "panel", 10), vec!["src/ui/panel.lua"]);
        let lua = find_files(&ws, &[], "lua", 10);
        assert!(lua.contains(&"main.lua".to_string()) && lua.contains(&"src/util.lua".to_string()));
        assert!(!lua.iter().any(|f| f.contains("node_modules")));
        assert_eq!(find_files(&ws, &[], "rdm", 10), vec!["README.md"], "letters in order");
        assert!(find_files(&ws, &[], "zzz", 10).is_empty());
        assert_eq!(find_files(&ws, &[], "", 2).len(), 2);
        assert_eq!(find_files(&ws, &[], "main", 10)[0], "main.lua");
    }

    #[test]
    fn search_text_finds_lines_skips_binary_and_ignored_and_honours_limit() {
        let (_d, ws) = project();
        let hits = search_text(&ws, &[], "NEEDLE", 10);
        assert_eq!(hits, vec!["main.lua:2: local Needle = 1", "src/ui/panel.lua:1: -- needle again"]);
        assert_eq!(search_text(&ws, &[], "needle", 1).len(), 1);
        assert!(search_text(&ws, &[], "  ", 10).is_empty());
    }

    #[test]
    fn user_ignore_names_are_honoured() {
        let (_d, ws) = project();
        let f = find_files(&ws, &["src".to_string()], "lua", 10);
        assert_eq!(f, vec!["main.lua"]);
    }
}
