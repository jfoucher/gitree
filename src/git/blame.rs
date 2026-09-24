//! `git blame --porcelain` parsing.

use super::{Git, GitResult};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct BlameCommit {
    pub author: String,
    pub author_time: i64,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct BlameLine {
    pub oid: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Blame {
    pub lines: Vec<BlameLine>,
    pub commits: HashMap<String, BlameCommit>,
}

pub fn blame(git: &Git, path: &str, rev: Option<&str>) -> GitResult<Blame> {
    let mut args = vec!["blame", "--porcelain"];
    if let Some(r) = rev {
        args.push(r);
    }
    args.push("--");
    args.push(path);
    Ok(parse(&git.run(&args)?))
}

pub fn parse(out: &str) -> Blame {
    let mut b = Blame::default();
    let mut cur_oid = String::new();
    for line in out.lines() {
        if let Some(text) = line.strip_prefix('\t') {
            b.lines.push(BlameLine {
                oid: cur_oid.clone(),
                text: text.to_string(),
            });
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");
        if key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            cur_oid = key.to_string();
            b.commits.entry(cur_oid.clone()).or_default();
            continue;
        }
        let Some(c) = b.commits.get_mut(&cur_oid) else { continue };
        match key {
            "author" => c.author = val.to_string(),
            "author-time" => c.author_time = val.parse().unwrap_or(0),
            "summary" => c.summary = val.to_string(),
            _ => {}
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let a = "a".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Ann\nauthor-time 123\nsummary First\nfilename f\n\tline one\n{a} 2 2\n\tline two\n"
        );
        let b = parse(&out);
        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.lines[1].text, "line two");
        assert_eq!(b.commits[&a].author, "Ann");
        assert_eq!(b.commits[&a].summary, "First");
    }
}
