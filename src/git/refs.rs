//! Branches, remote branches, tags, stashes, remotes and submodules.

use super::{Git, GitResult};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefInfo {
    /// Full ref name, e.g. `refs/heads/main`.
    pub full: String,
    /// Short name, e.g. `main`, `origin/main`, `v1.0`.
    pub name: String,
    pub kind: RefKind,
    /// Commit the ref points to (peeled for annotated tags).
    pub oid: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub upstream_gone: bool,
    pub is_head: bool,
    pub subject: String,
}

impl RefInfo {
    /// For remote refs: the remote name (`origin`).
    pub fn remote(&self) -> Option<&str> {
        if self.kind == RefKind::Remote {
            self.name.split('/').next()
        } else {
            None
        }
    }

    /// For remote refs: branch name without remote prefix.
    pub fn remote_branch(&self) -> Option<&str> {
        if self.kind == RefKind::Remote {
            self.name.split_once('/').map(|(_, b)| b)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Refs {
    pub refs: Vec<RefInfo>,
    /// HEAD commit (None on an unborn branch).
    pub head_oid: Option<String>,
    /// Current branch short name (None when detached).
    pub head_branch: Option<String>,
}

impl Refs {
    pub fn locals(&self) -> impl Iterator<Item = &RefInfo> {
        self.refs.iter().filter(|r| r.kind == RefKind::Local)
    }
    pub fn remotes(&self) -> impl Iterator<Item = &RefInfo> {
        self.refs
            .iter()
            .filter(|r| r.kind == RefKind::Remote && !r.name.ends_with("/HEAD"))
    }
    pub fn tags(&self) -> impl Iterator<Item = &RefInfo> {
        self.refs.iter().filter(|r| r.kind == RefKind::Tag)
    }
    pub fn current(&self) -> Option<&RefInfo> {
        self.refs.iter().find(|r| r.is_head)
    }
    pub fn find_local(&self, name: &str) -> Option<&RefInfo> {
        self.locals().find(|r| r.name == name)
    }

    /// Map commit -> refs pointing at it, used for history badges.
    pub fn by_oid(&self) -> HashMap<String, Vec<RefInfo>> {
        let mut m: HashMap<String, Vec<RefInfo>> = HashMap::new();
        for r in &self.refs {
            if r.kind == RefKind::Remote && r.name.ends_with("/HEAD") {
                continue;
            }
            m.entry(r.oid.clone()).or_default().push(r.clone());
        }
        m
    }

    /// A string that changes whenever any ref moves (to skip needless log reloads).
    pub fn signature(&self) -> String {
        let mut s = self.head_oid.clone().unwrap_or_default();
        s.push_str(self.head_branch.as_deref().unwrap_or(""));
        for r in &self.refs {
            s.push_str(&r.full);
            s.push_str(&r.oid);
        }
        s
    }
}

const FMT: &str = "%(refname)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(HEAD)%00%(contents:subject)";

pub fn refs(git: &Git) -> GitResult<Refs> {
    let out = git.run(&[
        "for-each-ref",
        &format!("--format={FMT}"),
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ])?;
    let mut refs = parse(&out);
    refs.head_oid = git
        .run(&["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string());
    refs.head_branch = git
        .run(&["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string());
    Ok(refs)
}

pub fn parse(out: &str) -> Refs {
    let mut refs = Refs::default();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 7 {
            continue;
        }
        let full = f[0];
        let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
            (RefKind::Local, n)
        } else if let Some(n) = full.strip_prefix("refs/remotes/") {
            (RefKind::Remote, n)
        } else if let Some(n) = full.strip_prefix("refs/tags/") {
            (RefKind::Tag, n)
        } else {
            continue;
        };
        let oid = if f[2].is_empty() { f[1] } else { f[2] };
        let (mut ahead, mut behind, mut gone) = (0, 0, false);
        for part in f[4].split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                behind = n.parse().unwrap_or(0);
            } else if part == "gone" {
                gone = true;
            }
        }
        refs.refs.push(RefInfo {
            full: full.to_string(),
            name: name.to_string(),
            kind,
            oid: oid.to_string(),
            upstream: (!f[3].is_empty()).then(|| f[3].to_string()),
            ahead,
            behind,
            upstream_gone: gone,
            is_head: f[5] == "*",
            subject: f[6].to_string(),
        });
    }
    refs
}

#[derive(Debug, Clone)]
pub struct Stash {
    /// `stash@{n}`
    pub name: String,
    pub oid: String,
    pub message: String,
}

pub fn stashes(git: &Git) -> GitResult<Vec<Stash>> {
    let out = git.run(&["stash", "list", "--format=%gd%x00%H%x00%gs"])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\0').collect();
            (f.len() >= 3).then(|| Stash {
                name: f[0].to_string(),
                oid: f[1].to_string(),
                message: f[2].to_string(),
            })
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

pub fn remotes(git: &Git) -> GitResult<Vec<Remote>> {
    let out = git.run(&["remote", "-v"])?;
    let mut list: Vec<Remote> = Vec::new();
    for l in out.lines() {
        let mut it = l.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let idx = match list.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                list.push(Remote {
                    name: name.to_string(),
                    fetch_url: String::new(),
                    push_url: String::new(),
                });
                list.len() - 1
            }
        };
        if kind == "(fetch)" {
            list[idx].fetch_url = url.to_string();
        } else {
            list[idx].push_url = url.to_string();
        }
    }
    Ok(list)
}

#[derive(Debug, Clone)]
pub struct Submodule {
    pub path: String,
    /// ' ' up to date, '-' not initialized, '+' different commit, 'U' conflicts
    pub state: char,
}

pub fn submodules(git: &Git) -> Vec<Submodule> {
    if !git.workdir.join(".gitmodules").exists() {
        return Vec::new();
    }
    let Ok(out) = git.run(&["submodule", "status"]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let state = l.chars().next()?;
            let rest = &l[1..];
            let mut it = rest.split_whitespace();
            let _oid = it.next()?;
            let path = it.next()?.to_string();
            Some(Submodule { path, state })
        })
        .collect()
}

/// Subtrees registered by Gitree in the repo config (`gitree.subtree.<prefix>.*`).
#[derive(Debug, Clone)]
pub struct Subtree {
    pub prefix: String,
    pub url: String,
    pub branch: String,
}

pub fn subtrees(git: &Git) -> Vec<Subtree> {
    let Ok(out) = git.run(&["config", "--local", "--get-regexp", r"^gitree\.subtree\."]) else {
        return Vec::new();
    };
    let mut map: Vec<Subtree> = Vec::new();
    for l in out.lines() {
        let Some((key, val)) = l.split_once(' ') else {
            continue;
        };
        let key = &key["gitree.subtree.".len()..];
        let Some((prefix, field)) = key.rsplit_once('.') else {
            continue;
        };
        let i = match map.iter().position(|s| s.prefix == prefix) {
            Some(i) => i,
            None => {
                map.push(Subtree {
                    prefix: prefix.to_string(),
                    url: String::new(),
                    branch: String::new(),
                });
                map.len() - 1
            }
        };
        match field {
            "url" => map[i].url = val.to_string(),
            "branch" => map[i].branch = val.to_string(),
            _ => {}
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_for_each_ref() {
        let out = "refs/heads/main\0aaa\0\0origin/main\0ahead 2, behind 3\0*\0Msg\n\
refs/heads/feature/x\0bbb\0\0origin/feature/x\0gone\0 \0Other\n\
refs/remotes/origin/HEAD\0aaa\0\0\0\0 \0Msg\n\
refs/remotes/origin/main\0aaa\0\0\0\0 \0Msg\n\
refs/tags/v1\0ttt\0ccc\0\0\0 \0Tag msg\n";
        let r = parse(out);
        assert_eq!(r.refs.len(), 5);
        let main = &r.refs[0];
        assert!(main.is_head);
        assert_eq!((main.ahead, main.behind), (2, 3));
        assert!(r.refs[1].upstream_gone);
        assert_eq!(r.remotes().count(), 1);
        assert_eq!(r.refs[3].remote(), Some("origin"));
        assert_eq!(r.refs[3].remote_branch(), Some("main"));
        assert_eq!(r.refs[4].oid, "ccc");
        assert_eq!(r.by_oid()["aaa"].len(), 2);
    }
}
