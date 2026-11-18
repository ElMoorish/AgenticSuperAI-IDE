//! SEARCH/REPLACE edit engine (Doc B "resilient apply_diff", Doc A Aider-style edit format).
//! - blocks are located against the ORIGINAL file, then applied bottom-up, so block order
//!   does not matter and one block cannot shift another;
//! - matching tries exact lines, then trailing-whitespace-insensitive, then fully trimmed;
//! - an optional `:start_line:N` hint disambiguates repeated snippets.

#[derive(Debug, PartialEq)]
pub struct Block {
    pub search: String,
    pub replace: String,
    pub start_line: Option<usize>,
}

const S: &str = "<<<<<<< SEARCH";
const M: &str = "=======";
const R: &str = ">>>>>>> REPLACE";

pub fn parse_blocks(s: &str) -> Result<Vec<Block>, String> {
    let mut blocks = Vec::new();
    let mut lines = s.lines().map(|l| l.trim_end_matches('\r')).peekable();
    while let Some(l) = lines.next() {
        if l.trim() != S {
            continue;
        }
        let mut start_line = None;
        if let Some(next) = lines.peek() {
            if let Some(n) = next.trim().strip_prefix(":start_line:") {
                start_line = n.trim().parse().ok();
                lines.next();
            }
        }
        let (mut search, mut replace) = (Vec::new(), Vec::new());
        let mut in_replace = false;
        let mut closed = false;
        for l in lines.by_ref() {
            if !in_replace && l.trim() == M {
                in_replace = true;
            } else if l.trim() == R {
                closed = true;
                break;
            } else if in_replace {
                replace.push(l);
            } else {
                search.push(l);
            }
        }
        if !in_replace || !closed {
            return Err("malformed block: expected SEARCH, =======, REPLACE markers".into());
        }
        blocks.push(Block { search: search.join("\n"), replace: replace.join("\n"), start_line });
    }
    if blocks.is_empty() {
        return Err("no SEARCH/REPLACE blocks found. Use exactly these marker lines:\n<<<<<<< SEARCH\nold lines\n=======\nnew lines\n>>>>>>> REPLACE".into());
    }
    Ok(blocks)
}

fn matches_at(file: &[String], i: usize, pat: &[String], norm: fn(&str) -> String) -> bool {
    i + pat.len() <= file.len() && pat.iter().enumerate().all(|(k, p)| norm(&file[i + k]) == norm(p))
}

fn locate(file: &[String], b: &Block) -> Result<(usize, usize), String> {
    let pat: Vec<String> = b.search.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    if pat.iter().all(|l| l.trim().is_empty()) {
        return Err("empty SEARCH block".into());
    }
    let norms: [fn(&str) -> String; 3] = [|s| s.to_string(), |s| s.trim_end().to_string(), |s| s.trim().to_string()];
    for norm in norms {
        let cands: Vec<usize> = (0..file.len()).filter(|&i| matches_at(file, i, &pat, norm)).collect();
        match (cands.len(), b.start_line) {
            (0, _) => continue,
            (1, _) => return Ok((cands[0], pat.len())),
            (_, Some(h)) => {
                let best = *cands.iter().min_by_key(|&&i| (i as i64 + 1 - h as i64).abs()).unwrap();
                return Ok((best, pat.len()));
            }
            (n, None) => return Err(format!("SEARCH block matches {n} places; add :start_line: or more context")),
        }
    }
    Err(format!("SEARCH block not found: {:?}", pat.first().map(|s| s.trim()).unwrap_or("")))
}

pub fn apply(content: &str, blocks: &[Block]) -> Result<String, String> {
    let eol = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = content.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let mut spans: Vec<(usize, usize, &Block)> = Vec::new();
    for b in blocks {
        let (start, len) = locate(&lines, b)?;
        spans.push((start, len, b));
    }
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    for w in spans.windows(2) {
        if w[1].0 + w[1].1 > w[0].0 {
            return Err("two SEARCH blocks overlap".into());
        }
    }
    for (start, len, b) in spans {
        let rep: Vec<String> = if b.replace.is_empty() { vec![] } else { b.replace.split('\n').map(String::from).collect() };
        lines.splice(start..start + len, rep);
    }
    Ok(lines.join(eol))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blk(s: &str, r: &str, h: Option<usize>) -> Block {
        Block { search: s.into(), replace: r.into(), start_line: h }
    }

    #[test]
    fn parses_blocks_with_hint() {
        let t = "<<<<<<< SEARCH\n:start_line:3\nfoo\n=======\nbar\n>>>>>>> REPLACE\n";
        assert_eq!(parse_blocks(t).unwrap(), vec![blk("foo", "bar", Some(3))]);
    }

    #[test]
    fn malformed_is_error() {
        assert!(parse_blocks("<<<<<<< SEARCH\nfoo\n").is_err());
        assert!(parse_blocks("nothing").is_err());
    }

    #[test]
    fn exact_replace() {
        assert_eq!(apply("a\nb\nc\n", &[blk("b", "B", None)]).unwrap(), "a\nB\nc\n");
    }

    #[test]
    fn whitespace_fuzzy_match() {
        let out = apply("fn x() {\n    let a = 1;\n}\n", &[blk("let a = 1;", "let a = 2;", None)]).unwrap();
        assert!(out.contains("let a = 2;"));
    }

    #[test]
    fn ambiguous_needs_hint() {
        let f = "x\nfoo\ny\nfoo\n";
        assert!(apply(f, &[blk("foo", "bar", None)]).is_err());
        assert_eq!(apply(f, &[blk("foo", "bar", Some(4))]).unwrap(), "x\nfoo\ny\nbar\n");
    }

    #[test]
    fn order_invariant_multi_block() {
        let f = "1\n2\n3\n4\n";
        let a = apply(f, &[blk("2", "two\nextra", None), blk("4", "four", None)]).unwrap();
        let b = apply(f, &[blk("4", "four", None), blk("2", "two\nextra", None)]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "1\ntwo\nextra\n3\nfour\n");
    }

    #[test]
    fn not_found_errors() {
        assert!(apply("a\n", &[blk("zzz", "y", None)]).is_err());
    }

    #[test]
    fn crlf_preserved() {
        assert_eq!(apply("a\r\nb\r\n", &[blk("b", "c", None)]).unwrap(), "a\r\nc\r\n");
    }

    #[test]
    fn delete_block() {
        assert_eq!(apply("a\nb\nc\n", &[blk("b", "", None)]).unwrap(), "a\nc\n");
    }
}
