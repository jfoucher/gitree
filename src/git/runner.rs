//! Thin wrapper around the `git` command line.
//!
//! Every git invocation goes through [`Git`], which sets a predictable
//! environment (C locale, no terminal prompts, our own askpass helper) so
//! output can be parsed reliably.

use crate::i18n::{gettext, gettext_f};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Error produced by a failed git invocation.
#[derive(Debug, Clone)]
pub struct GitError {
    pub command: String,
    pub stderr: String,
    pub stdout: String,
    #[allow(dead_code)]
    pub code: Option<i32>,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        write!(f, "{}\n\n{}", self.command, msg)
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

/// How git should ask for credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// Use the Gitree askpass dialog.
    Interactive,
    /// Never prompt; fail instead (used for background fetches).
    Never,
}

#[derive(Debug, Clone)]
pub struct Git {
    pub workdir: PathBuf,
}

impl Git {
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
        }
    }

    /// Builds a `git` command with the standard environment.
    pub fn command<S: AsRef<str>>(&self, args: &[S], prompt: Prompt) -> Command {
        self.command_with_env(args, prompt, &[])
    }

    fn command_with_env<S: AsRef<str>>(
        &self,
        args: &[S],
        prompt: Prompt,
        extra_env: &[(String, String)],
    ) -> Command {
        let mut cmd = base_command(prompt);
        cmd.current_dir(&self.workdir);
        for a in args {
            cmd.arg(a.as_ref());
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        crate::host::wrap(cmd, true)
    }

    /// Runs git and returns stdout on success.
    pub fn run<S: AsRef<str>>(&self, args: &[S]) -> GitResult<String> {
        self.run_with_input(args, None)
    }

    /// Runs git, returning stdout even when the exit code is 1 (used for
    /// commands like `diff --no-index` whose exit code signals differences).
    pub fn run_allow_1<S: AsRef<str>>(&self, args: &[S]) -> GitResult<String> {
        let out = self.output(args, None, Prompt::Never)?;
        if out.code == Some(0) || out.code == Some(1) {
            Ok(out.stdout)
        } else {
            Err(out.into_error(describe(args)))
        }
    }

    pub fn run_with_input<S: AsRef<str>>(
        &self,
        args: &[S],
        input: Option<&[u8]>,
    ) -> GitResult<String> {
        let out = self.output(args, input, Prompt::Never)?;
        if out.code == Some(0) {
            Ok(out.stdout)
        } else {
            Err(out.into_error(describe(args)))
        }
    }

    /// Returns true when the command exits with status 0.
    pub fn check<S: AsRef<str>>(&self, args: &[S]) -> bool {
        self.output(args, None, Prompt::Never)
            .map(|o| o.code == Some(0))
            .unwrap_or(false)
    }

    /// Raw bytes of stdout (for binary blobs such as images).
    pub fn run_bytes<S: AsRef<str>>(&self, args: &[S]) -> GitResult<Vec<u8>> {
        let mut cmd = self.command(args, Prompt::Never);
        let out = cmd.output().map_err(|e| spawn_error(args, e))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(GitError {
                command: describe(args),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                stdout: String::new(),
                code: out.status.code(),
            })
        }
    }

    fn output<S: AsRef<str>>(
        &self,
        args: &[S],
        input: Option<&[u8]>,
        prompt: Prompt,
    ) -> GitResult<Output> {
        let mut cmd = self.command(args, prompt);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = cmd.spawn().map_err(|e| spawn_error(args, e))?;
        if let Some(data) = input
            && let Some(mut stdin) = child.stdin.take() {
                let data = data.to_vec();
                // Write from a thread so a large input can't deadlock
                // against a full stdout pipe.
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&data);
                });
            }
        let out = child.wait_with_output().map_err(|e| spawn_error(args, e))?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        })
    }

    /// Runs a (possibly long) command, calling `on_line` with every line of
    /// progress written to stderr/stdout. The child handle is stored in
    /// `handle` so the caller can kill it to cancel.
    pub fn run_streaming<S: AsRef<str>>(
        &self,
        args: &[S],
        prompt: Prompt,
        extra_env: &[(String, String)],
        handle: Arc<Mutex<Option<Child>>>,
        on_line: impl Fn(String) + Send + Clone + 'static,
    ) -> GitResult<String> {
        let mut cmd = self.command_with_env(args, prompt, extra_env);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| spawn_error(args, e))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *handle.lock().unwrap() = Some(child);

        let out_cb = on_line.clone();
        let out_thread = std::thread::spawn(move || {
            let mut all = Vec::new();
            if let Some(s) = stdout {
                read_lines(s, &mut all, out_cb);
            }
            String::from_utf8_lossy(&all).into_owned()
        });
        let err_thread = std::thread::spawn(move || {
            let mut all = Vec::new();
            if let Some(s) = stderr {
                read_lines(s, &mut all, on_line);
            }
            String::from_utf8_lossy(&all).into_owned()
        });
        let stdout = out_thread.join().unwrap_or_default();
        let stderr = err_thread.join().unwrap_or_default();

        let status = {
            let mut guard = handle.lock().unwrap();
            match guard.take() {
                Some(mut c) => c.wait().ok(),
                None => None,
            }
        };
        let code = status.and_then(|s| s.code());
        if code == Some(0) {
            Ok(stdout)
        } else {
            Err(GitError {
                command: describe(args),
                stderr: if code.is_none() {
                    format!("{stderr}\n{}", gettext("Cancelled."))
                } else {
                    stderr
                },
                stdout,
                code,
            })
        }
    }

    /// Path to the repository's git dir (handles worktrees & submodules).
    pub fn git_dir(&self) -> PathBuf {
        match self.run(&["rev-parse", "--absolute-git-dir"]) {
            Ok(s) => PathBuf::from(s.trim()),
            Err(_) => self.workdir.join(".git"),
        }
    }
}

struct Output {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Output {
    fn into_error(self, command: String) -> GitError {
        GitError {
            command,
            stderr: self.stderr,
            stdout: self.stdout,
            code: self.code,
        }
    }
}

fn spawn_error<S: AsRef<str>>(args: &[S], e: std::io::Error) -> GitError {
    GitError {
        command: describe(args),
        stderr: gettext_f("Failed to run git: {error}", &[("error", &e.to_string())]),
        stdout: String::new(),
        code: None,
    }
}

/// Human readable command line, e.g. `git push origin main`.
pub fn describe<S: AsRef<str>>(args: &[S]) -> String {
    let mut s = String::from("git");
    for a in args {
        let a = a.as_ref();
        s.push(' ');
        if a.contains(' ') || a.is_empty() {
            s.push('"');
            s.push_str(a);
            s.push('"');
        } else {
            s.push_str(a);
        }
    }
    s
}

/// Splits a stream on `\n` and `\r` (git progress uses carriage returns).
fn read_lines(mut r: impl Read, all: &mut Vec<u8>, mut f: impl FnMut(String)) {
    let mut buf = [0u8; 4096];
    let mut line = Vec::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        all.extend_from_slice(&buf[..n]);
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !line.is_empty() {
                    f(String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }
    if !line.is_empty() {
        f(String::from_utf8_lossy(&line).into_owned());
    }
}

/// A `git` command with Gitree's environment, not bound to a directory.
/// Pass it through [`crate::host::wrap`] before spawning.
fn base_command(prompt: Prompt) -> Command {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "color.ui=false",
        "-c",
        "core.quotepath=false",
        "-c",
        "advice.detachedHead=false",
    ]);
    cmd.env("LC_ALL", "C.UTF-8");
    cmd.env("LANGUAGE", "C");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    // Never pop up an editor from git itself.
    cmd.env("GIT_EDITOR", "true");
    cmd.env_remove("GIT_DIR");
    cmd.env_remove("GIT_WORK_TREE");
    match prompt {
        Prompt::Interactive => {
            if let Some(exe) = crate::host::askpass_program() {
                cmd.env("GIT_ASKPASS", &exe);
                cmd.env("SSH_ASKPASS", &exe);
                cmd.env("SSH_ASKPASS_REQUIRE", "force");
                cmd.env(crate::ASKPASS_ENV, "1");
            }
        }
        Prompt::Never => {
            cmd.env("GIT_ASKPASS", "true");
            cmd.env("SSH_ASKPASS_REQUIRE", "never");
            cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
    }
    cmd
}

/// Runs a git command outside of any repository (e.g. `clone`, `config --global`).
pub fn run_global<S: AsRef<str>>(args: &[S]) -> GitResult<String> {
    let mut cmd = base_command(Prompt::Never);
    for a in args {
        cmd.arg(a.as_ref());
    }
    let out = crate::host::wrap(cmd, true).output().map_err(|e| spawn_error(args, e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(GitError {
            command: describe(args),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            code: out.status.code(),
        })
    }
}

/// Returns the top-level work tree containing `path`, if it is inside a repo.
pub fn find_toplevel(path: &Path) -> Option<PathBuf> {
    let git = Git::new(path);
    git.run(&["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| PathBuf::from(s.trim()))
}
