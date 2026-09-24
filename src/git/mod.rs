//! Git backend: everything here shells out to the `git` CLI and parses
//! its machine-readable output. Nothing in this module depends on GTK.

pub mod blame;
pub mod diff;
pub mod flow;
pub mod graph;
pub mod log;
pub mod rebase;
pub mod refs;
mod runner;
pub mod state;
pub mod status;

pub use runner::*;

pub fn config_get(git: Option<&Git>, key: &str) -> Option<String> {
    let r = match git {
        Some(g) => g.run(&["config", "--get", key]),
        None => run_global(&["config", "--global", "--get", key]),
    };
    r.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Returns true for text that looks like a full or abbreviated object id.
pub fn looks_like_oid(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod integration_tests;
