//! File system watcher that tells a repository tab to refresh.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Something in the work tree or index changed.
    Worktree,
    /// Refs / HEAD changed (commits, checkouts, fetches).
    Refs,
}

/// Keeps the OS watcher alive; dropping it stops watching (and ends the
/// debounce thread).
pub struct Watcher {
    _inner: RecommendedWatcher,
}

const DEBOUNCE: Duration = Duration::from_millis(350);

/// Classifies a changed path; `None` means it should be ignored.
fn classify(path: &Path, workdir: &Path, git_dir: &Path) -> Option<Change> {
    if let Ok(rel) = path.strip_prefix(git_dir) {
        let s = rel.to_string_lossy();
        if s.ends_with(".lock")
            || s.starts_with("objects")
            || s.starts_with("logs")
            || s.starts_with("lfs")
            || s.starts_with("modules")
            || s.starts_with("gitree")
            || s.contains("GITREE_")
            || s.is_empty()
        {
            return None;
        }
        if s == "index" {
            return Some(Change::Worktree);
        }
        return Some(Change::Refs);
    }
    if path.strip_prefix(workdir).is_ok() {
        // Nested .git directories (submodules) are noisy.
        if path.components().any(|c| c.as_os_str() == ".git") {
            return None;
        }
        return Some(Change::Worktree);
    }
    None
}

/// Starts watching; `notify` is called (from a background thread) with the
/// kinds of change seen in each debounced batch.
pub fn watch(
    workdir: PathBuf,
    git_dir: PathBuf,
    notify: impl Fn(Vec<Change>) + Send + 'static,
) -> Option<Watcher> {
    let (tx, rx) = mpsc::channel::<Event>();
    let mut inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            // Reads (git opening files/dirs) must not trigger refreshes.
            if !matches!(ev.kind, EventKind::Access(_)) {
                let _ = tx.send(ev);
            }
        }
    })
    .ok()?;
    inner.watch(&workdir, RecursiveMode::Recursive).ok()?;
    if !git_dir.starts_with(&workdir) {
        let _ = inner.watch(&git_dir, RecursiveMode::Recursive);
    }

    std::thread::spawn(move || {
        let debug = std::env::var_os("GITREE_DEBUG_REFRESH").is_some();
        // Ends when the watcher (and thus the sender) is dropped.
        while let Ok(first) = rx.recv() {
            let mut paths: Vec<PathBuf> = first.paths;
            let deadline = Instant::now() + DEBOUNCE;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(ev) => paths.extend(ev.paths),
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            paths.sort();
            paths.dedup();
            let mut kinds = Vec::new();
            let mut worktree_paths = Vec::new();
            for p in paths {
                if debug {
                    eprintln!("gitree: fs event {}", p.display());
                }
                match classify(&p, &workdir, &git_dir) {
                    Some(Change::Worktree) if p.strip_prefix(&git_dir).is_err() => worktree_paths.push(p),
                    Some(k) if !kinds.contains(&k) => kinds.push(k),
                    _ => {}
                }
            }
            if !worktree_paths.is_empty()
                && !kinds.contains(&Change::Worktree)
                && !all_ignored(&workdir, &worktree_paths)
            {
                kinds.push(Change::Worktree);
            }
            if !kinds.is_empty() {
                notify(kinds);
            }
        }
    });
    Some(Watcher { _inner: inner })
}

/// True if every path is ignored by .gitignore (e.g. build output).
fn all_ignored(workdir: &Path, paths: &[PathBuf]) -> bool {
    let mut input = String::new();
    for p in paths.iter().take(200) {
        input.push_str(&p.to_string_lossy());
        input.push('\0');
    }
    let git = crate::git::Git::new(workdir);
    match git.run_with_input(&["check-ignore", "-z", "--stdin"], Some(input.as_bytes())) {
        Ok(out) => out.split('\0').filter(|s| !s.is_empty()).count() >= paths.len().min(200),
        Err(_) => false,
    }
}
