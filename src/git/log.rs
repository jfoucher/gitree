//! Streaming `git log` reader.

use super::{Git, GitError, GitResult, Prompt};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Stdio};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub subject: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.oid[..self.oid.len().min(7)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogScope {
    /// All branches, remote branches and tags.
    #[default]
    All,
    /// Local branches and tags only.
    Local,
    /// Only what is reachable from HEAD.
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogOrder {
    #[default]
    Date,
    Topo,
}

#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub scope: LogScope,
    pub order: LogOrder,
    /// Extra arguments such as `--grep=foo`, `--author=bar`, `-Sfoo`.
    pub extra: Vec<String>,
    pub paths: Vec<String>,
    pub follow: bool,
    /// Explicit revision (e.g. a single branch) instead of the scope.
    pub rev: Option<String>,
    pub has_head: bool,
}

const FMT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ce%x1f%ct%x1f%s";

impl LogQuery {
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["log".into(), FMT.into()];
        match self.order {
            LogOrder::Date => a.push("--date-order".into()),
            LogOrder::Topo => a.push("--topo-order".into()),
        }
        a.extend(self.extra.iter().cloned());
        if self.follow {
            a.push("--follow".into());
        }
        if let Some(rev) = &self.rev {
            a.push(rev.clone());
        } else {
            match self.scope {
                LogScope::All => {
                    a.extend(["--branches", "--remotes", "--tags"].map(String::from));
                }
                LogScope::Local => {
                    a.extend(["--branches", "--tags"].map(String::from));
                }
                LogScope::Current => {}
            }
            if self.has_head {
                a.push("HEAD".into());
            }
        }
        a.push("--".into());
        a.extend(self.paths.iter().cloned());
        a
    }
}

/// Iterator over commits produced by a running `git log`.
pub struct LogReader {
    child: Child,
    reader: BufReader<ChildStdout>,
    buf: Vec<u8>,
    started: bool,
}

impl LogReader {
    pub fn spawn(git: &Git, q: &LogQuery) -> GitResult<Self> {
        let args = q.args();
        let mut cmd = git.command(&args, Prompt::Never);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| GitError {
            command: super::describe(&args),
            stderr: e.to_string(),
            stdout: String::new(),
            code: None,
        })?;
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Self {
            child,
            reader: BufReader::with_capacity(1 << 16, stdout),
            buf: Vec::with_capacity(512),
            started: false,
        })
    }
}

impl Iterator for LogReader {
    type Item = Commit;

    fn next(&mut self) -> Option<Commit> {
        loop {
            self.buf.clear();
            let n = self.reader.read_until(0x1e, &mut self.buf).ok()?;
            if n == 0 {
                return None;
            }
            if !self.started {
                // Output starts with a separator; skip the empty first record.
                self.started = true;
                if self.buf == [0x1e] {
                    continue;
                }
            }
            if self.buf.last() == Some(&0x1e) {
                self.buf.pop();
            }
            let rec = String::from_utf8_lossy(&self.buf);
            if let Some(c) = parse_record(rec.trim_end_matches('\n')) {
                return Some(c);
            }
        }
    }
}

impl Drop for LogReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn parse_record(rec: &str) -> Option<Commit> {
    let f: Vec<&str> = rec.split('\x1f').collect();
    if f.len() < 9 {
        return None;
    }
    Some(Commit {
        oid: f[0].to_string(),
        parents: f[1].split_whitespace().map(String::from).collect(),
        author_name: f[2].to_string(),
        author_email: f[3].to_string(),
        author_time: f[4].parse().unwrap_or(0),
        committer_name: f[5].to_string(),
        committer_email: f[6].to_string(),
        commit_time: f[7].parse().unwrap_or(0),
        subject: f[8].to_string(),
    })
}

/// Full commit message.
pub fn message(git: &Git, oid: &str) -> GitResult<String> {
    git.run(&["show", "-s", "--format=%B", oid])
}

/// Single commit metadata.
pub fn commit(git: &Git, rev: &str) -> GitResult<Commit> {
    let out = git.run(&["show", "-s", &FMT.replace("%x1e", ""), rev])?;
    parse_record(out.trim_end()).ok_or_else(|| GitError {
        command: format!("git show {rev}"),
        stderr: "Could not parse commit".into(),
        stdout: out,
        code: None,
    })
}

/// Commits from `base` (exclusive) to HEAD, oldest first (for interactive rebase).
pub fn range_oldest_first(git: &Git, base: Option<&str>) -> GitResult<Vec<Commit>> {
    let range = match base {
        Some(b) => format!("{b}..HEAD"),
        None => "HEAD".to_string(),
    };
    let out = git.run(&[
        "log",
        "--reverse",
        "--topo-order",
        &FMT.replace("%x1e", ""),
        "-z",
        &range,
    ])?;
    Ok(out
        .split('\0')
        .filter_map(|r| parse_record(r.trim_matches('\n')))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_record() {
        let c = parse_record("abc\x1fp1 p2\x1fAnn\x1fa@x\x1f100\x1fBob\x1fb@x\x1f200\x1fHello world").unwrap();
        assert_eq!(c.parents, vec!["p1", "p2"]);
        assert_eq!(c.author_time, 100);
        assert_eq!(c.subject, "Hello world");
    }

    #[test]
    fn builds_args() {
        let q = LogQuery {
            has_head: true,
            ..Default::default()
        };
        let a = q.args();
        assert!(a.contains(&"--remotes".to_string()));
        assert_eq!(a.last().unwrap(), "--");
    }
}
