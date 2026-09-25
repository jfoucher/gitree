//! Interactive rebase todo generation.

use crate::i18n::N_;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseAction {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    pub const ALL: [RebaseAction; 6] = [
        RebaseAction::Pick,
        RebaseAction::Reword,
        RebaseAction::Edit,
        RebaseAction::Squash,
        RebaseAction::Fixup,
        RebaseAction::Drop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RebaseAction::Pick => N_("Pick"),
            RebaseAction::Reword => N_("Reword"),
            RebaseAction::Edit => N_("Edit"),
            RebaseAction::Squash => N_("Squash"),
            RebaseAction::Fixup => N_("Fixup"),
            RebaseAction::Drop => N_("Drop"),
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            RebaseAction::Pick | RebaseAction::Reword => "pick",
            RebaseAction::Edit => "edit",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RebaseItem {
    pub oid: String,
    pub subject: String,
    pub action: RebaseAction,
    /// Replacement message for the commit resulting from this item (and
    /// any squash/fixup items that follow it).
    pub new_message: Option<String>,
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Writes the todo list (and message files) into `dir` and returns the
/// todo file path. Custom messages are applied with `exec git commit --amend`.
pub fn write_todo(items: &[RebaseItem], dir: &Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let todo = build_todo(items, dir, |path, msg| std::fs::write(path, msg))?;
    let path = dir.join("git-rebase-todo");
    std::fs::write(&path, todo)?;
    Ok(path)
}

pub fn build_todo(
    items: &[RebaseItem],
    dir: &Path,
    mut write_msg: impl FnMut(&Path, &str) -> std::io::Result<()>,
) -> std::io::Result<String> {
    let mut out = String::new();
    let mut pending: Option<String> = None;
    for (i, it) in items.iter().enumerate() {
        out.push_str(&format!("{} {} {}\n", it.action.keyword(), it.oid, it.subject));
        if it.action == RebaseAction::Drop {
            continue;
        }
        if let Some(m) = &it.new_message {
            pending = Some(m.clone());
        }
        let next_joins = items[i + 1..]
            .iter()
            .find(|n| n.action != RebaseAction::Drop)
            .is_some_and(|n| matches!(n.action, RebaseAction::Squash | RebaseAction::Fixup));
        if !next_joins
            && let Some(m) = pending.take() {
                let file = dir.join(format!("msg-{i}.txt"));
                write_msg(&file, &m)?;
                out.push_str(&format!(
                    "exec git commit --amend --allow-empty --no-verify -q -F {}\n",
                    shell_quote(&file.to_string_lossy())
                ));
            }
    }
    Ok(out)
}

/// Value for `GIT_SEQUENCE_EDITOR` that installs our prepared todo file.
pub fn sequence_editor(todo: &Path) -> String {
    format!("cp {}", shell_quote(&todo.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(oid: &str, action: RebaseAction, msg: Option<&str>) -> RebaseItem {
        RebaseItem {
            oid: oid.into(),
            subject: format!("subj {oid}"),
            action,
            new_message: msg.map(String::from),
        }
    }

    #[test]
    fn squash_group_message_goes_after_group() {
        let items = vec![
            item("a", RebaseAction::Pick, Some("New A")),
            item("b", RebaseAction::Squash, None),
            item("c", RebaseAction::Drop, None),
            item("d", RebaseAction::Reword, Some("New D")),
        ];
        let mut written = Vec::new();
        let todo = build_todo(&items, Path::new("/tmp/x"), |p, m| {
            written.push((p.to_path_buf(), m.to_string()));
            Ok(())
        })
        .unwrap();
        let lines: Vec<&str> = todo.lines().collect();
        assert_eq!(lines[0], "pick a subj a");
        assert_eq!(lines[1], "squash b subj b");
        assert!(lines[2].starts_with("exec git commit --amend"));
        assert_eq!(lines[3], "drop c subj c");
        assert_eq!(lines[4], "pick d subj d");
        assert!(lines[5].contains("msg-3.txt"));
        assert_eq!(written.len(), 2);
        assert_eq!(written[0].1, "New A");
    }
}
