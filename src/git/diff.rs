//! Unified diff parsing and partial patch generation (hunk / line staging).

use super::{Git, GitResult};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Text without the leading `+`/`-`/space.
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The complete `@@ ... @@ section` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// Lines from `diff --git` up to (not including) the first hunk.
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub new_file: bool,
    pub deleted: bool,
    pub renamed: bool,
    pub mode_change: bool,
}

impl FileDiff {
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }

    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let end = rest.find(" @@")?;
    let (old, new) = rest[..end].split_once(" +")?;
    let (os, oc) = parse_range(old);
    let (ns, nc) = parse_range(new);
    Some((os, oc, ns, nc))
}

fn strip_path(p: &str) -> Option<String> {
    let p = p.trim_end_matches('\t');
    if p == "/dev/null" {
        return None;
    }
    let p = p.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(p);
    Some(
        p.strip_prefix("a/")
            .or_else(|| p.strip_prefix("b/"))
            .unwrap_or(p)
            .to_string(),
    )
}

pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut in_hunk = false;
    let (mut old_no, mut new_no) = (0u32, 0u32);

    for line in text.split('\n') {
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            let mut f = FileDiff {
                header: vec![line.to_string()],
                ..Default::default()
            };
            // Fallback paths from the diff line (overridden by ---/+++).
            if let Some(rest) = line.strip_prefix("diff --git ")
                && let Some((a, b)) = rest.split_once(" b/") {
                    f.old_path = strip_path(a);
                    f.new_path = Some(b.to_string());
                }
            cur = Some(f);
            in_hunk = false;
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };

        if line.starts_with("@@ ")
            && let Some((os, oc, ns, nc)) = parse_hunk_header(line) {
                f.hunks.push(Hunk {
                    old_start: os,
                    old_count: oc,
                    new_start: ns,
                    new_count: nc,
                    header: line.to_string(),
                    lines: Vec::new(),
                });
                old_no = os;
                new_no = ns;
                in_hunk = true;
                continue;
            }

        if in_hunk {
            let h = f.hunks.last_mut().unwrap();
            let (kind, rest) = match line.as_bytes().first() {
                Some(b'+') => (LineKind::Add, &line[1..]),
                Some(b'-') => (LineKind::Del, &line[1..]),
                Some(b' ') => (LineKind::Context, &line[1..]),
                Some(b'\\') => (LineKind::NoNewline, line),
                // Empty line: end of output or an empty context line from
                // tools that strip trailing spaces.
                None => continue,
                _ => {
                    in_hunk = false;
                    f.header.push(line.to_string());
                    continue;
                }
            };
            let (o, n) = match kind {
                LineKind::Add => {
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Del => {
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
                LineKind::NoNewline => (None, None),
            };
            h.lines.push(DiffLine {
                kind,
                text: rest.to_string(),
                old_no: o,
                new_no: n,
            });
            continue;
        }

        // Extended header lines.
        if line.is_empty() {
            continue;
        }
        f.header.push(line.to_string());
        if let Some(p) = line.strip_prefix("--- ") {
            f.old_path = strip_path(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            f.new_path = strip_path(p);
        } else if line.starts_with("new file mode") {
            f.new_file = true;
        } else if line.starts_with("deleted file mode") {
            f.deleted = true;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.renamed = true;
            f.old_path = Some(p.to_string());
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.new_path = Some(p.to_string());
        } else if line.starts_with("old mode") {
            f.mode_change = true;
        } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
            f.binary = true;
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    for f in &mut files {
        if f.new_file {
            f.old_path = None;
        }
        if f.deleted {
            f.new_path = None;
        }
    }
    files
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchMode {
    /// Apply forward (stage lines from the unstaged diff).
    Forward,
    /// Apply with `--reverse` (unstage lines from the staged diff, or
    /// discard lines from the unstaged diff).
    Reverse,
}

/// Selection within one file: hunk index -> selected line indices
/// (`None` = the whole hunk).
pub type Selection = Vec<(usize, Option<BTreeSet<usize>>)>;

/// Builds a patch containing only the selected hunks/lines.
///
/// For `Forward`, unselected additions are dropped and unselected deletions
/// become context. For `Reverse` it is the other way around, because the
/// patch will be applied backwards on top of the "new" side.
pub fn build_patch(file: &FileDiff, selection: &Selection, mode: PatchMode) -> String {
    let mut out = String::new();
    for h in &file.header {
        if h.starts_with("---") || h.starts_with("+++") {
            continue;
        }
        out.push_str(h);
        out.push('\n');
    }
    out.push_str(&match &file.old_path {
        Some(p) if !file.new_file => format!("--- a/{p}\n"),
        _ => "--- /dev/null\n".to_string(),
    });
    out.push_str(&match &file.new_path {
        Some(p) if !file.deleted => format!("+++ b/{p}\n"),
        _ => "+++ /dev/null\n".to_string(),
    });

    let mut delta: i64 = 0;
    for (hi, sel) in selection {
        let Some(h) = file.hunks.get(*hi) else { continue };
        let mut body = String::new();
        let (mut oc, mut nc) = (0u32, 0u32);
        let mut last_kept = true;
        for (li, l) in h.lines.iter().enumerate() {
            let selected = sel.as_ref().is_none_or(|s| s.contains(&li));
            let emit = |body: &mut String, prefix: char| {
                body.push(prefix);
                body.push_str(&l.text);
                body.push('\n');
            };
            match l.kind {
                LineKind::Context => {
                    emit(&mut body, ' ');
                    oc += 1;
                    nc += 1;
                    last_kept = true;
                }
                LineKind::Add => {
                    if selected {
                        emit(&mut body, '+');
                        nc += 1;
                        last_kept = true;
                    } else if mode == PatchMode::Reverse {
                        emit(&mut body, ' ');
                        oc += 1;
                        nc += 1;
                        last_kept = true;
                    } else {
                        last_kept = false;
                    }
                }
                LineKind::Del => {
                    if selected {
                        emit(&mut body, '-');
                        oc += 1;
                        last_kept = true;
                    } else if mode == PatchMode::Forward {
                        emit(&mut body, ' ');
                        oc += 1;
                        nc += 1;
                        last_kept = true;
                    } else {
                        last_kept = false;
                    }
                }
                LineKind::NoNewline => {
                    if last_kept {
                        body.push_str(&l.text);
                        body.push('\n');
                    }
                }
            }
        }
        if oc == nc && !body.lines().any(|l| l.starts_with('+') || l.starts_with('-')) {
            continue;
        }
        // In unified diffs an empty range names the line *before* it, so
        // the old/new start lines are offset by one when a side is empty.
        let old_start = if h.old_count == 0 && oc > 0 {
            h.old_start + 1
        } else if oc == 0 && h.old_count > 0 {
            h.old_start.saturating_sub(1)
        } else {
            h.old_start
        };
        let mut new_start = old_start as i64 + delta;
        if oc == 0 && nc > 0 {
            new_start += 1;
        } else if nc == 0 && oc > 0 {
            new_start -= 1;
        }
        let new_start = new_start.max(0) as u32;
        let section = h
            .header
            .find(" @@")
            .map(|i| &h.header[i + 3..])
            .unwrap_or("");
        out.push_str(&format!(
            "@@ -{old_start},{oc} +{new_start},{nc} @@{section}\n"
        ));
        out.push_str(&body);
        delta += nc as i64 - oc as i64;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTarget {
    /// Stage lines from the unstaged diff.
    Stage,
    /// Unstage lines from the staged diff.
    Unstage,
    /// Discard lines from the unstaged diff (modifies the work tree).
    Discard,
}

/// Applies the selected hunks/lines of `file` to the index or work tree.
/// `untracked` files are first added with intent-to-add so they can be
/// staged partially.
pub fn apply_selection(
    git: &Git,
    file: &FileDiff,
    sel: &Selection,
    target: ApplyTarget,
    untracked: bool,
    opts: DiffOptions,
) -> GitResult<()> {
    let mut file = file.clone();
    if untracked && target == ApplyTarget::Stage {
        let path = file.path().to_string();
        git.run(&["add", "-N", "--", &path])?;
        if let Some(f) = unstaged(git, &path, opts)?.into_iter().next() {
            file = f;
        }
    }
    let (mode, mut args): (PatchMode, Vec<&str>) = match target {
        ApplyTarget::Stage => (PatchMode::Forward, vec!["apply", "--cached"]),
        ApplyTarget::Unstage => (PatchMode::Reverse, vec!["apply", "--cached", "--reverse"]),
        ApplyTarget::Discard => (PatchMode::Reverse, vec!["apply", "--reverse"]),
    };
    let patch = build_patch(&file, sel, mode);
    args.extend(["--recount", "--unidiff-zero", "--whitespace=nowarn", "-"]);
    git.run_with_input(&args, Some(patch.as_bytes()))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffOptions {
    pub context: u32,
    pub ignore_whitespace: bool,
}

fn common_args(opts: DiffOptions) -> Vec<String> {
    let mut a = vec![
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        format!("-U{}", opts.context),
    ];
    if opts.ignore_whitespace {
        a.push("-w".into());
    }
    a
}

/// Diff of unstaged changes (index -> worktree) for one path.
pub fn unstaged(git: &Git, path: &str, opts: DiffOptions) -> GitResult<Vec<FileDiff>> {
    let mut a = vec!["diff".to_string()];
    a.extend(common_args(opts));
    a.extend(["--".to_string(), path.to_string()]);
    Ok(parse(&git.run(&a)?))
}

/// Diff of staged changes (HEAD -> index) for one path.
pub fn staged(git: &Git, path: &str, opts: DiffOptions) -> GitResult<Vec<FileDiff>> {
    let mut a = vec!["diff".to_string(), "--cached".to_string(), "-M".to_string()];
    a.extend(common_args(opts));
    a.extend(["--".to_string(), path.to_string()]);
    Ok(parse(&git.run(&a)?))
}

/// Diff showing an untracked file as entirely added.
pub fn untracked(git: &Git, path: &str, opts: DiffOptions) -> GitResult<Vec<FileDiff>> {
    let mut a = vec!["diff".to_string(), "--no-index".to_string()];
    a.extend(common_args(opts));
    a.extend(["--".to_string(), "/dev/null".to_string(), path.to_string()]);
    let mut files = parse(&git.run_allow_1(&a)?);
    for f in &mut files {
        f.new_path = Some(path.to_string());
    }
    Ok(files)
}

/// Diff introduced by a commit (against its first parent), optionally limited to paths.
pub fn commit(git: &Git, oid: &str, paths: &[String], opts: DiffOptions) -> GitResult<Vec<FileDiff>> {
    let mut a = vec![
        "diff-tree".to_string(),
        "-p".to_string(),
        "-r".to_string(),
        "-M".to_string(),
        "--root".to_string(),
        "--diff-merges=first-parent".to_string(),
    ];
    a.extend(common_args(opts));
    a.push(oid.to_string());
    a.push("--".into());
    a.extend(paths.iter().cloned());
    Ok(parse(&git.run(&a)?))
}

/// Diff between two arbitrary revisions.
pub fn between(git: &Git, from: &str, to: &str, paths: &[String], opts: DiffOptions) -> GitResult<Vec<FileDiff>> {
    let mut a = vec!["diff".to_string(), "-M".to_string()];
    a.extend(common_args(opts));
    a.push(from.to_string());
    a.push(to.to_string());
    a.push("--".into());
    a.extend(paths.iter().cloned());
    Ok(parse(&git.run(&a)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// A, M, D, R, C, T
    pub status: char,
    pub path: String,
    pub old_path: Option<String>,
}

fn parse_name_status(out: &str) -> Vec<ChangedFile> {
    let mut v = Vec::new();
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    while let Some(st) = it.next() {
        let status = st.chars().next().unwrap_or('M');
        if status == 'R' || status == 'C' {
            let old = it.next().unwrap_or_default().to_string();
            let new = it.next().unwrap_or_default().to_string();
            v.push(ChangedFile {
                status,
                path: new,
                old_path: Some(old),
            });
        } else if let Some(p) = it.next() {
            v.push(ChangedFile {
                status,
                path: p.to_string(),
                old_path: None,
            });
        }
    }
    v
}

/// Files changed by a commit (first-parent).
pub fn commit_files(git: &Git, oid: &str) -> GitResult<Vec<ChangedFile>> {
    let out = git.run(&[
        "diff-tree",
        "--no-commit-id",
        "--name-status",
        "-r",
        "-z",
        "-M",
        "--root",
        "--diff-merges=first-parent",
        oid,
    ])?;
    Ok(parse_name_status(&out))
}

pub fn files_between(git: &Git, from: &str, to: &str) -> GitResult<Vec<ChangedFile>> {
    let out = git.run(&["diff", "--name-status", "-z", "-M", from, to])?;
    Ok(parse_name_status(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/f.txt b/f.txt
index 111..222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,4 @@ fn main
 one
-two
+TWO
+extra
 three
-four
@@ -10,2 +10,3 @@
 ten
+eleven
 twelve
";

    #[test]
    fn parses_hunks() {
        let f = &parse(DIFF)[0];
        assert_eq!(f.path(), "f.txt");
        assert_eq!(f.hunks.len(), 2);
        let h = &f.hunks[0];
        assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (1, 4, 1, 4));
        assert_eq!(h.lines.len(), 6);
        assert_eq!(h.lines[2].new_no, Some(2));
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(f.hunks[1].lines[1].new_no, Some(11));
    }

    #[test]
    fn forward_line_selection() {
        let f = &parse(DIFF)[0];
        // Select only "+TWO" (line 2) in hunk 0.
        let sel: Selection = vec![(0, Some([2].into_iter().collect()))];
        let p = build_patch(f, &sel, PatchMode::Forward);
        assert!(p.contains("@@ -1,4 +1,5 @@ fn main\n one\n two\n+TWO\n three\n four\n"), "{p}");
    }

    #[test]
    fn reverse_line_selection() {
        let f = &parse(DIFF)[0];
        // Unstage only "-two" (line 1): unselected additions become context.
        let sel: Selection = vec![(0, Some([1].into_iter().collect()))];
        let p = build_patch(f, &sel, PatchMode::Reverse);
        assert!(p.contains("@@ -1,5 +1,4 @@ fn main\n one\n-two\n TWO\n extra\n three\n"), "{p}");
    }

    #[test]
    fn whole_hunk_and_delta() {
        let f = &parse(DIFF)[0];
        let sel: Selection = vec![(0, None), (1, None)];
        let p = build_patch(f, &sel, PatchMode::Forward);
        assert!(p.contains("@@ -1,4 +1,4 @@"));
        assert!(p.contains("@@ -10,2 +10,3 @@"));
    }

    #[test]
    fn parses_new_and_binary() {
        let d = "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 000..111\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\ndiff --git a/img.png b/img.png\nindex 1..2 100644\nBinary files a/img.png and b/img.png differ\n";
        let files = parse(d);
        assert_eq!(files.len(), 2);
        assert!(files[0].new_file);
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[0].path(), "new.txt");
        assert!(files[1].binary);
        assert_eq!(files[1].path(), "img.png");
    }

    #[test]
    fn name_status() {
        let v = parse_name_status("M\0a.rs\0R100\0old.rs\0new.rs\0A\0b.rs\0");
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(v[1].path, "new.rs");
    }
}
