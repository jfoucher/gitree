//! Parsing of `git status --porcelain=v2 -z --branch`.

use super::{Git, GitResult};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub orig_path: Option<String>,
    /// Index status (`.` means unmodified), porcelain XY first letter.
    pub index: char,
    /// Worktree status, porcelain XY second letter.
    pub worktree: char,
    pub conflicted: bool,
    pub untracked: bool,
    pub ignored: bool,
    pub submodule: bool,
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        !self.untracked && !self.ignored && !self.conflicted && self.index != '.'
    }

    pub fn is_unstaged(&self) -> bool {
        self.untracked || self.ignored || self.conflicted || self.worktree != '.'
    }

    /// Status letter shown in the staged list.
    pub fn staged_code(&self) -> char {
        self.index
    }

    /// Status letter shown in the unstaged list.
    pub fn unstaged_code(&self) -> char {
        if self.conflicted {
            'U'
        } else if self.untracked {
            '?'
        } else if self.ignored {
            '!'
        } else {
            self.worktree
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub head_oid: Option<String>,
    /// None when detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<StatusEntry>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_staged())
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|e| e.conflicted)
    }

    pub fn change_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.ignored).count()
    }
}

pub fn status(git: &Git, include_ignored: bool) -> GitResult<Status> {
    let mut args = vec![
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
    ];
    if include_ignored {
        args.push("--ignored=matching");
    }
    Ok(parse(&git.run(&args)?))
}

pub fn parse(out: &str) -> Status {
    let mut st = Status::default();
    let mut fields = out.split('\0').peekable();
    while let Some(rec) = fields.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" if val != "(initial)" => st.head_oid = Some(val.to_string()),
                "branch.head" if val != "(detached)" => st.branch = Some(val.to_string()),
                "branch.upstream" => st.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = rec.as_bytes()[0];
        match kind {
            b'1' | b'2' | b'u' => {
                // Number of space separated fields before the path.
                let n = match kind {
                    b'1' => 8,
                    b'2' => 9,
                    _ => 10,
                };
                let parts: Vec<&str> = rec.splitn(n + 1, ' ').collect();
                if parts.len() < n + 1 {
                    continue;
                }
                let xy: Vec<char> = parts[1].chars().collect();
                let sub = parts[2];
                let mut e = StatusEntry {
                    path: parts[n].to_string(),
                    index: xy.first().copied().unwrap_or('.'),
                    worktree: xy.get(1).copied().unwrap_or('.'),
                    conflicted: kind == b'u',
                    submodule: sub.starts_with('S'),
                    ..Default::default()
                };
                if kind == b'2' {
                    e.orig_path = fields.next().map(|s| s.to_string());
                }
                st.entries.push(e);
            }
            b'?' | b'!' => {
                st.entries.push(StatusEntry {
                    path: rec[2..].to_string(),
                    index: '.',
                    worktree: if kind == b'?' { '?' } else { '!' },
                    untracked: kind == b'?',
                    ignored: kind == b'!',
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let out = "# branch.oid abc123\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 A. N... 000000 100644 100644 000 bbb new file.txt\0\
2 R. N... 100644 100644 100644 aaa aaa R100 b.rs\0a.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? untracked.txt\0! target/x\0";
        let st = parse(out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.entries.len(), 6);
        assert_eq!(st.entries[1].path, "new file.txt");
        assert!(st.entries[1].is_staged() && !st.entries[1].is_unstaged());
        assert!(st.entries[0].is_unstaged() && !st.entries[0].is_staged());
        assert_eq!(st.entries[2].orig_path.as_deref(), Some("a.rs"));
        assert_eq!(st.entries[2].path, "b.rs");
        assert!(st.entries[3].conflicted);
        assert!(st.entries[4].untracked);
        assert!(st.entries[5].ignored);
    }

    #[test]
    fn parses_initial_and_detached() {
        let st = parse("# branch.oid (initial)\0# branch.head (detached)\0");
        assert!(st.head_oid.is_none());
        assert!(st.branch.is_none());
    }
}
