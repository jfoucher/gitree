//! Detection of in-progress operations (merge, rebase, cherry-pick, ...).

use crate::i18n::{gettext, gettext_f};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OpState {
    #[default]
    None,
    Merge,
    Rebase {
        interactive: bool,
        step: u32,
        total: u32,
        /// The rebase stopped on an `edit` command (not a conflict).
        editing: bool,
    },
    CherryPick,
    Revert,
    Bisect,
}

impl OpState {
    pub fn is_none(&self) -> bool {
        matches!(self, OpState::None)
    }

    pub fn label(&self) -> String {
        match self {
            OpState::None => String::new(),
            OpState::Merge => gettext("A merge is in progress."),
            OpState::Rebase { step, total, editing, .. } => {
                let args = [("step", &*step.to_string()), ("total", &*total.to_string())];
                if *editing {
                    gettext_f("Rebase stopped for editing (step {step} of {total}). Amend the commit, then continue.", &args)
                } else {
                    gettext_f("A rebase is in progress (step {step} of {total}).", &args)
                }
            }
            OpState::CherryPick => gettext("A cherry-pick is in progress."),
            OpState::Revert => gettext("A revert is in progress."),
            OpState::Bisect => gettext("A bisect is in progress."),
        }
    }

    /// Name of the git sub-command used for --continue / --abort.
    pub fn command(&self) -> Option<&'static str> {
        match self {
            OpState::Merge => Some("merge"),
            OpState::Rebase { .. } => Some("rebase"),
            OpState::CherryPick => Some("cherry-pick"),
            OpState::Revert => Some("revert"),
            OpState::Bisect => Some("bisect"),
            OpState::None => None,
        }
    }
}

fn read_num(p: &Path) -> u32 {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn op_state(git_dir: &Path) -> OpState {
    let rm = git_dir.join("rebase-merge");
    let ra = git_dir.join("rebase-apply");
    if rm.is_dir() {
        let editing = rm.join("amend").exists() && !git_dir.join("MERGE_MSG").exists();
        return OpState::Rebase {
            interactive: rm.join("interactive").exists(),
            step: read_num(&rm.join("msgnum")),
            total: read_num(&rm.join("end")),
            editing,
        };
    }
    if ra.is_dir() {
        return OpState::Rebase {
            interactive: false,
            step: read_num(&ra.join("next")),
            total: read_num(&ra.join("last")),
            editing: false,
        };
    }
    if git_dir.join("MERGE_HEAD").exists() {
        return OpState::Merge;
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return OpState::CherryPick;
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return OpState::Revert;
    }
    if git_dir.join("BISECT_LOG").exists() {
        return OpState::Bisect;
    }
    OpState::None
}

/// Message prepared by git for the pending merge/cherry-pick commit.
pub fn merge_message(git_dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(git_dir.join("MERGE_MSG")).ok()?;
    Some(
        s.lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
    )
}
