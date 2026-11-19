//! Small rotating file log and the "copy diagnostics" report.
//!
//! The log keeps `max_files` files of at most `max_bytes` each (`asa.log` is the newest, then
//! `asa.1.log` ...), so it can never grow without bound. Diagnostics are meant to be pasted into
//! a bug report: the user's home folder path and any credentials in URLs are removed first.

use crate::settings::Settings;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct RotatingLog {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    lock: Mutex<()>,
}

impl RotatingLog {
    pub fn new(dir: PathBuf, max_bytes: u64, max_files: usize) -> RotatingLog {
        RotatingLog { dir, max_bytes: max_bytes.max(1), max_files: max_files.max(1), lock: Mutex::new(()) }
    }

    /// The defaults from the design: 5 files of 2 MB.
    pub fn standard(dir: PathBuf) -> RotatingLog {
        RotatingLog::new(dir, 2 * 1024 * 1024, 5)
    }

    fn path(&self, n: usize) -> PathBuf {
        self.dir.join(if n == 0 { "asa.log".to_string() } else { format!("asa.{n}.log") })
    }

    /// Never panics and never returns an error: logging must not be able to break the app.
    pub fn write(&self, level: &str, msg: &str) {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let _ = fs::create_dir_all(&self.dir);
        // keep one record on one line so tail/diagnostics stay line based
        let one = msg.replace('\r', "").replace('\n', " | ");
        let line = format!("{} {level} {one}\n", now_secs());
        let cur = self.path(0);
        let size = fs::metadata(&cur).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size + line.len() as u64 > self.max_bytes {
            self.rotate();
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&cur) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn rotate(&self) {
        let _ = fs::remove_file(self.path(self.max_files - 1));
        for n in (0..self.max_files - 1).rev() {
            let from = self.path(n);
            if from.exists() {
                let _ = fs::rename(&from, self.path(n + 1));
            }
        }
    }

    /// The last `n` lines across the files, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<String> = Vec::new();
        for i in 0..self.max_files {
            if out.len() >= n {
                break;
            }
            let Ok(bytes) = fs::read(self.path(i)) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            lines.reverse(); // newest first within the file
            out.extend(lines);
        }
        out.truncate(n);
        out.reverse();
        out
    }

    pub fn file_count(&self) -> usize {
        (0..self.max_files).filter(|i| self.path(*i).exists()).count()
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Replace the home folder (both slash styles, any case on Windows) and the user name with `~`.
pub fn redact(text: &str, home: Option<&str>) -> String {
    let mut out = text.to_string();
    if let Some(h) = home.map(str::trim).filter(|h| h.len() > 3) {
        for variant in [h.to_string(), h.replace('\\', "/"), h.replace('/', "\\"), h.replace('\\', "\\\\")] {
            out = replace_ci(&out, &variant, "~");
        }
        if let Some(user) = h.rsplit(['\\', '/']).find(|s| !s.is_empty()) {
            if user.len() >= 3 {
                out = replace_ci(&out, user, "<user>");
            }
        }
    }
    out
}

fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return hay.to_string();
    }
    let (lh, ln) = (hay.to_lowercase(), needle.to_lowercase());
    if lh.len() != hay.len() {
        return hay.replace(needle, with); // non-ASCII case folding changed lengths: exact match only
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lh[i..].find(&ln) {
        out.push_str(&hay[i..i + p]);
        out.push_str(with);
        i += p + ln.len();
    }
    out.push_str(&hay[i..]);
    out
}

/// `user:pass@` in a URL is removed.
fn strip_userinfo(url: &str) -> String {
    if let Some(i) = url.find("://") {
        let (scheme, rest) = url.split_at(i + 3);
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if let Some(at) = rest[..end].rfind('@') {
            return format!("{scheme}{}", &rest[at + 1..]);
        }
    }
    url.to_string()
}

pub struct DiagnosticsInput<'a> {
    pub app_version: &'a str,
    pub os: &'a str,
    pub agent_exe: Option<&'a str>,
    pub settings: &'a Settings,
    pub log_tail: Vec<String>,
    pub home: Option<&'a str>,
}

/// A plain-text report for a bug report. Contains no home path, credentials or file contents.
pub fn diagnostics(i: &DiagnosticsInput<'_>) -> String {
    let s = i.settings;
    let mut r = String::new();
    r.push_str("ASA IDE diagnostics\n");
    r.push_str(&format!("version: {}\nos: {}\n", i.app_version, i.os));
    r.push_str(&format!("agent: {}\n", i.agent_exe.unwrap_or("not found")));
    r.push_str("settings:\n");
    r.push_str(&format!("  endpoint: {}\n  model: {}\n  theme: {}\n  font_size: {}\n", strip_userinfo(&s.endpoint), s.model, s.theme, s.font_size));
    r.push_str(&format!("  ignore patterns: {}\n  recent folders: {}\n", s.ignore.len(), s.recent.len()));
    r.push_str("log (newest last):\n");
    for l in &i.log_tail {
        r.push_str("  ");
        r.push_str(l);
        r.push('\n');
    }
    redact(&r, i.home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotates() {
        let d = tempfile::tempdir().unwrap();
        let log = RotatingLog::new(d.path().to_path_buf(), 200, 3);
        for i in 0..60 {
            log.write("INFO", &format!("line number {i:03} padding padding padding"));
        }
        assert_eq!(log.file_count(), 3, "never more than max_files");
        for n in 0..3 {
            let p = d.path().join(if n == 0 { "asa.log".into() } else { format!("asa.{n}.log") });
            assert!(fs::metadata(&p).unwrap().len() <= 200 + 80, "{p:?} is bounded");
        }
        let tail = log.tail(5);
        assert_eq!(tail.len(), 5);
        assert!(tail.last().unwrap().contains("line number 059"), "{tail:?}");
        assert!(tail[0].contains("line number 055"), "{tail:?}");
        assert!(!log.tail(10_000).iter().any(|l| l.contains("line number 000")), "oldest lines were dropped");
    }

    #[test]
    fn multi_line_messages_stay_on_one_line() {
        let d = tempfile::tempdir().unwrap();
        let log = RotatingLog::standard(d.path().to_path_buf());
        log.write("ERROR", "first\r\nsecond");
        assert_eq!(log.tail(10).len(), 1);
    }

    #[test]
    fn diagnostics_contain_no_home_path_secrets() {
        let home = r"C:\Users\Jane Doe";
        let s = Settings {
            endpoint: "http://bob:hunter2@localhost:11434/v1".into(),
            recent: vec![r"C:\Users\Jane Doe\code\secret-project".into()],
            ..Settings::default()
        };
        let rep = diagnostics(&DiagnosticsInput {
            app_version: "0.1.0",
            os: "windows",
            agent_exe: Some(r"c:\users\jane doe\appdata\asa\agent-core.exe"),
            settings: &s,
            log_tail: vec![
                r"1 ERROR read C:/Users/Jane Doe/code/a.txt failed".into(),
                r"2 INFO opened C:\\Users\\Jane Doe\\x".into(),
            ],
            home: Some(home),
        });
        for bad in ["Jane", "hunter2", "bob:", r"C:\Users", "C:/Users"] {
            assert!(!rep.contains(bad), "leaked {bad:?} in:\n{rep}");
        }
        assert!(rep.contains("localhost:11434"));
        assert!(rep.contains("agent-core.exe"));
        assert!(rep.contains("recent folders: 1"), "counts, not paths");
    }
}
