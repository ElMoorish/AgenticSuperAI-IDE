//! Shell command policy and output shaping (Doc B "cap terminal output", Doc A Windows sandboxing).
//! Commands are never passed to a shell by the agent: they are split into program + args and
//! sent to the editor's terminal API. Shell metacharacters are rejected outright, so pipes,
//! redirects and command chaining cannot be used to smuggle something past the allowlist.

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Build/check/read-only commands: run without a prompt.
    Safe,
    /// Unknown but not forbidden: needs the user's approval.
    Ask,
    /// Never run.
    Deny(&'static str),
}

const DENY_PROGRAMS: &[&str] = &[
    "rm", "del", "erase", "rmdir", "rd", "format", "shutdown", "reg", "sudo", "curl", "wget", "iwr", "irm",
    "powershell", "pwsh", "cmd", "bash", "sh", "remove-item", "diskpart", "taskkill", "chmod", "icacls",
];
const META: &[&str] = &[";", "&", "|", ">", "<", "`", "$(", "\n", "\r"];

/// (program, subcommands that are safe). Empty list = any arguments are safe.
const SAFE: &[(&str, &[&str])] = &[
    ("cargo", &["check", "build", "test", "clippy", "--version", "fmt"]),
    ("npm", &["run", "test", "--version"]),
    ("npx", &["tsc", "eslint"]),
    ("tsc", &[]),
    ("cmake", &["--build", "--version"]),
    ("ninja", &[]),
    ("make", &[]),
    ("luacheck", &[]),
    ("rg", &[]),
    ("git", &["status", "diff", "log", "show", "branch", "--version"]),
];

/// Split on whitespace, honouring double quotes.
pub fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn classify(cmdline: &str) -> Verdict {
    if let Some(m) = META.iter().find(|m| cmdline.contains(**m)) {
        let _ = m;
        return Verdict::Deny("shell operators (; & | > < ` $() newlines) are not allowed");
    }
    let parts = split_args(cmdline);
    let Some(prog) = parts.first() else { return Verdict::Deny("empty command") };
    let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog).to_lowercase();
    let base = base.trim_end_matches(".exe").trim_end_matches(".cmd");
    if DENY_PROGRAMS.contains(&base) {
        return Verdict::Deny("this program is on the deny list");
    }
    if parts.iter().any(|a| a == "--force" || a == "-rf" || a == "--hard") {
        return Verdict::Ask;
    }
    match SAFE.iter().find(|(p, _)| *p == base) {
        Some((_, subs)) if subs.is_empty() => Verdict::Safe,
        Some((_, subs)) => match parts.get(1) {
            Some(s) if subs.contains(&s.as_str()) => Verdict::Safe,
            _ => Verdict::Ask,
        },
        None => Verdict::Ask,
    }
}

/// Only run post-edit diagnostics for files a compiler/linter actually looks at.
pub fn is_source(path: &str) -> bool {
    const EXT: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "c", "cc", "cpp", "cxx", "h", "hpp", "lua"];
    path.rsplit('.').next().map(|e| EXT.contains(&e.to_lowercase().as_str())).unwrap_or(false) && path.contains('.')
}

/// Keep the head and tail of long output so the model sees the first errors and the summary.
pub fn truncate_output(s: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= head + tail {
        return s.trim_end().to_string();
    }
    let omitted = lines.len() - head - tail;
    format!("{}\n[... {omitted} lines omitted ...]\n{}", lines[..head].join("\n"), lines[lines.len() - tail..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quotes() {
        assert_eq!(split_args(r#"cargo test "a b" -q"#), vec!["cargo", "test", "a b", "-q"]);
        assert_eq!(split_args("  a   b "), vec!["a", "b"]);
    }

    #[test]
    fn safe_build_commands() {
        assert_eq!(classify("cargo check --message-format short"), Verdict::Safe);
        assert_eq!(classify("npx tsc --noEmit"), Verdict::Safe);
        assert_eq!(classify("git status"), Verdict::Safe);
        assert_eq!(classify("cmake --build build"), Verdict::Safe);
    }

    #[test]
    fn unknown_or_risky_asks() {
        assert_eq!(classify("cargo install ripgrep"), Verdict::Ask);
        assert_eq!(classify("git push"), Verdict::Ask);
        assert_eq!(classify("python script.py"), Verdict::Ask);
        assert_eq!(classify("git log --force"), Verdict::Ask);
    }

    #[test]
    fn denies_destructive_and_operators() {
        assert!(matches!(classify("rm -rf /"), Verdict::Deny(_)));
        assert!(matches!(classify(r"C:\Windows\System32\cmd.exe /c dir"), Verdict::Deny(_)));
        assert!(matches!(classify("cargo check && rm x"), Verdict::Deny(_)));
        assert!(matches!(classify("git status > out.txt"), Verdict::Deny(_)));
        assert!(matches!(classify("curl http://x"), Verdict::Deny(_)));
        assert!(matches!(classify(""), Verdict::Deny(_)));
    }

    #[test]
    fn source_detection() {
        assert!(is_source("src/main.rs") && is_source("a/b.CPP") && is_source("x.lua"));
        assert!(!is_source("README.md") && !is_source("Makefile") && !is_source("rs"));
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        let s: String = (1..=100).map(|i| format!("l{i}\n")).collect();
        let t = truncate_output(&s, 3, 2);
        assert_eq!(t, "l1\nl2\nl3\n[... 95 lines omitted ...]\nl99\nl100");
        assert_eq!(truncate_output("a\nb\n", 3, 2), "a\nb");
    }
}
