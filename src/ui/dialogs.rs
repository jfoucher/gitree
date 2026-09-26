//! Repository actions and their dialogs (Pull, Push, Fetch, Branch,
//! Merge, Stash, Tag, Reset, ...). Every action is a `repo.<name>` GAction
//! with a string parameter, dispatched through [`dispatch`].

use super::staging::{decode_target, Side};
use super::form::{combo_value, text_of, Form};
use super::progress::OpOptions;
use super::repo_view::{RepoView, View};
use super::{ask_text, bg, confirm, copy_to_clipboard, show_error, spawn};
use crate::config;
use crate::git::state::OpState;
use crate::git::{self, Git};
use crate::i18n::{gettext, gettext_f, ngettext_f, pgettext};
use adw::prelude::*;
use gtk::gio;
use std::path::PathBuf;
use std::rc::Rc;

/// All actions registered on each repository view (all take a string).
pub const ACTIONS: &[&str] = &[
    "commit", "pull", "pull-ref", "push", "push-branch", "fetch", "remote-fetch", "remote-prune",
    "branch", "merge", "merge-ref", "merge-commit", "stash", "stash-apply", "stash-pop", "stash-drop",
    "stash-show", "stash-branch", "discard", "tag", "delete-tag", "push-tag", "flow", "flow-finish",
    "terminal", "files", "settings", "refresh", "show-status", "show-history", "show-search", "lfs",
    "add-submodule", "update-submodule", "sync-submodule", "open-submodule", "remove-submodule",
    "add-subtree", "subtree-pull", "subtree-push", "subtree-unlink", "apply-patch",
    "rebase-interactive-pick", "rebase-interactive", "clean", "gc", "op-continue", "op-abort",
    "op-skip", "checkout-ref", "checkout-commit", "rebase-onto", "delete-branch",
    "delete-remote-branch", "rename-branch", "track", "diff-ref", "copy-text", "copy-message",
    "reset-to", "revert", "cherry-pick", "archive", "patch", "custom-action", "file-stage",
    "file-unstage", "file-discard", "file-remove", "file-stop-tracking", "file-ignore", "file-open",
    "file-show", "file-difftool", "file-log", "file-blame", "file-blame-at", "file-checkout-at",
    "file-resolve-mine", "file-resolve-theirs", "file-mergetool", "file-mark-resolved",
    "file-mark-unresolved", "remote-edit", "remote-remove", "remote-add", "remote-copy-url",
    "open-remote", "debug-select-unstaged", "debug-select-staged", "debug-select-commit",
    "debug-select-lines", "debug-search",
];

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn net() -> OpOptions {
    OpOptions {
        network: true,
        ..Default::default()
    }
}

pub fn dispatch(rv: &Rc<RepoView>, name: &str, arg: String) {
    let rv = rv.clone();
    let name = name.to_string();
    spawn(async move { handle(&rv, &name, arg).await });
}

pub(super) async fn handle(rv: &Rc<RepoView>, name: &str, arg: String) {
    let parent = rv.widget.clone();
    match name {
        "commit" => {
            rv.show_view(View::Status);
            rv.file_status.focus_commit();
        }
        "pull" => pull_dialog(rv, None).await,
        "pull-ref" => pull_dialog(rv, Some(arg)).await,
        "push" => push_dialog(rv, None).await,
        "push-branch" => push_dialog(rv, Some(arg)).await,
        "fetch" => fetch_dialog(rv).await,
        "remote-fetch" => {
            rv.run_ops(&gettext("Fetch"), vec![s(&["fetch", "--progress", "--prune", &arg])], net())
                .await;
        }
        "remote-prune" => {
            rv.run_ops(&gettext("Prune"), vec![s(&["remote", "prune", &arg])], net()).await;
        }
        "branch" => branch_dialog(rv, &arg).await,
        "merge" => merge_dialog(rv, None).await,
        "merge-ref" => merge_dialog(rv, Some(arg)).await,
        "merge-commit" => {
            let short = &arg[..arg.len().min(10)];
            if confirm(&parent, &gettext("Merge Commit"), &gettext_f("Merge commit {commit} into the current branch?", &[("commit", short)]), &gettext("Merge"), false).await {
                rv.run_ops(&gettext("Merge"), vec![s(&["merge", "--no-edit", &arg])], OpOptions::default()).await;
            }
        }
        "stash" => stash_dialog(rv).await,
        "stash-apply" => {
            let form = Form::new(&gettext("Apply Stash"), &gettext("Apply"));
            form.description(&gettext_f("Apply {stash} to the working copy?", &[("stash", &arg)]));
            let del = form.switch(&gettext("Delete after applying"), "", false);
            let idx = form.switch(&gettext("Also restore the staged state (--index)"), "", false);
            form.focus_ok();
            if form.run(&parent).await {
                let mut c = s(&["stash", if del.is_active() { "pop" } else { "apply" }]);
                if idx.is_active() {
                    c.push("--index".into());
                }
                c.push(arg);
                rv.run_ops(&gettext("Apply Stash"), vec![c], OpOptions::default()).await;
            }
        }
        "stash-pop" => {
            rv.run_ops(&gettext("Pop Stash"), vec![s(&["stash", "pop", &arg])], OpOptions::default()).await;
        }
        "stash-drop" => {
            if confirm(&parent, &gettext("Delete Stash?"), &gettext_f("{stash} will be permanently deleted.", &[("stash", &arg)]), &gettext("Delete"), true).await {
                rv.run_ops(&gettext("Delete Stash"), vec![s(&["stash", "drop", &arg])], OpOptions::default()).await;
            }
        }
        "stash-show" => {
            let git = rv.git.clone();
            let a = arg.clone();
            match bg(move || git::log::commit(&git, &a)).await {
                Ok(c) => super::history::commit_window(rv, &gettext_f("Stash {stash}", &[("stash", &arg)]), c),
                Err(e) => show_error(&parent, &gettext("Could not show stash"), &e.to_string()),
            }
        }
        "stash-branch" => {
            if let Some(b) = ask_text(&parent, &gettext("Create Branch from Stash"), &gettext("The stash is applied on a new branch created from the commit it was based on."), "", &gettext("Create")).await {
                rv.run_ops(&gettext("Stash Branch"), vec![s(&["stash", "branch", &b, &arg])], OpOptions::default()).await;
            }
        }
        "discard" => discard_dialog(rv).await,
        "tag" => tag_dialog(rv, &arg).await,
        "delete-tag" => {
            let form = Form::new(&gettext("Delete Tag"), &gettext("Delete"));
            form.description(&gettext_f("Delete tag “{tag}”?", &[("tag", &arg)]));
            let remotes: Vec<String> = rv.snapshot().remotes.iter().map(|r| r.name.clone()).collect();
            let remote_sw = form.switch(&gettext("Also remove the tag from the remote"), "", false);
            let remote = form.combo(&gettext("Remote"), &remotes, rv.snapshot().default_remote().as_deref());
            remote.set_visible(!remotes.is_empty());
            remote_sw.set_visible(!remotes.is_empty());
            form.ok.add_css_class("destructive-action");
            form.focus_cancel();
            if form.run(&parent).await {
                let mut cmds = vec![s(&["tag", "-d", &arg])];
                if remote_sw.is_active() {
                    cmds.push(s(&["push", "--progress", &combo_value(&remote), &format!(":refs/tags/{arg}")]));
                }
                rv.run_ops(&gettext("Delete Tag"), cmds, OpOptions { network: remote_sw.is_active(), ..Default::default() }).await;
            }
        }
        "push-tag" => {
            let remotes: Vec<String> = rv.snapshot().remotes.iter().map(|r| r.name.clone()).collect();
            if remotes.is_empty() {
                show_error(&parent, &gettext("No remotes"), &gettext("Add a remote in Repository Settings first."));
                return;
            }
            let form = Form::new(&if arg.is_empty() { gettext("Push All Tags") } else { gettext("Push Tag") }, &gettext("Push"));
            let remote = form.combo(&gettext("Push to repository"), &remotes, rv.snapshot().default_remote().as_deref());
            form.focus_ok();
            if form.run(&parent).await {
                let r = combo_value(&remote);
                let c = if arg.is_empty() {
                    s(&["push", "--progress", &r, "--tags"])
                } else {
                    s(&["push", "--progress", &r, &format!("refs/tags/{arg}")])
                };
                rv.run_ops(&gettext("Push Tag"), vec![c], net()).await;
            }
        }
        "flow" => super::flow::show(rv).await,
        "flow-finish" => super::flow::finish(rv, &arg).await,
        "terminal" => {
            if let Err(e) = super::open_terminal(&rv.git.workdir) {
                show_error(&parent, &gettext("Could not open terminal"), &e);
            }
        }
        "files" => super::open_folder(&rv.git.workdir),
        "settings" => super::repo_settings::show(rv, 0),
        "refresh" => {
            rv.invalidate_log();
            rv.refresh();
        }
        "show-status" => rv.show_view(View::Status),
        "show-history" => rv.show_view(View::History),
        "show-search" => rv.show_view(View::Search),
        "lfs" => super::repo_settings::lfs_dialog(rv).await,
        "add-submodule" => {
            let form = Form::new(&gettext("Add Submodule"), &gettext("Add"));
            let url = form.entry(&gettext("Source URL"), "");
            let path = form.entry(&gettext("Local relative path"), "");
            let branch = form.entry(&gettext("Branch (optional)"), "");
            let (u2, p2) = (url.clone(), path.clone());
            url.connect_changed(move |e| {
                if p2.text().is_empty() || !p2.has_focus() {
                    p2.set_text(&super::browser::repo_name_from_url(&e.text()));
                }
            });
            form.watch(&url);
            form.validate(move || !u2.text().trim().is_empty());
            form.focus(&url);
            if form.run(&parent).await {
                let mut c = s(&["submodule", "add", "--progress"]);
                if !branch.text().trim().is_empty() {
                    c.push("-b".into());
                    c.push(branch.text().trim().to_string());
                }
                c.push(url.text().trim().to_string());
                if !path.text().trim().is_empty() {
                    c.push(path.text().trim().to_string());
                }
                rv.run_ops(&gettext("Add Submodule"), vec![c], net()).await;
            }
        }
        "update-submodule" => {
            let mut c = s(&["submodule", "update", "--init", "--recursive", "--progress"]);
            if !arg.is_empty() {
                c.push("--".into());
                c.push(arg);
            }
            rv.run_ops(&gettext("Update Submodules"), vec![c], net()).await;
        }
        "sync-submodule" => {
            rv.run_ops(&gettext("Sync Submodule"), vec![s(&["submodule", "sync", "--recursive", "--", &arg])], OpOptions::default()).await;
        }
        "open-submodule" => {
            let p = rv.git.workdir.join(&arg);
            if p.join(".git").exists() {
                (rv.open_repo)(p);
            } else {
                show_error(&parent, &gettext("Submodule not initialised"), &gettext("Use “Update” to initialise the submodule first."));
            }
        }
        "remove-submodule" => {
            if confirm(&parent, &gettext("Remove Submodule?"), &gettext_f("Remove submodule “{path}” from the repository?", &[("path", &arg)]), &gettext("Remove"), true).await {
                let gd = rv.git_dir.join("modules").join(&arg);
                let ok = rv
                    .run_ops(
                        &gettext("Remove Submodule"),
                        vec![s(&["submodule", "deinit", "-f", "--", &arg]), s(&["rm", "-f", "--", &arg])],
                        OpOptions::default(),
                    )
                    .await;
                if ok {
                    let _ = std::fs::remove_dir_all(gd);
                }
            }
        }
        "add-subtree" => subtree_dialog(rv).await,
        "subtree-pull" | "subtree-push" => {
            let st = rv.snapshot().subtrees.iter().find(|x| x.prefix == arg).cloned();
            let Some(st) = st else { return };
            let (verb, title) = if name == "subtree-pull" {
                ("pull", gettext("Subtree Pull"))
            } else {
                ("push", gettext("Subtree Push"))
            };
            let mut c = s(&["subtree", verb, &format!("--prefix={}", st.prefix), &st.url, &st.branch]);
            if verb == "pull" {
                c.push("--squash".into());
                c.push("-m".into());
                c.push(format!("Merge subtree {}", st.prefix));
            }
            rv.run_ops(&title, vec![c], net()).await;
        }
        "subtree-unlink" => {
            rv.run_ops(
                &gettext("Unlink Subtree"),
                vec![s(&["config", "--remove-section", &format!("gitree.subtree.{arg}")])],
                OpOptions::default(),
            )
            .await;
        }
        "apply-patch" => apply_patch_dialog(rv).await,
        "rebase-interactive-pick" => {
            let snap = rv.snapshot();
            let base = snap
                .refs
                .current()
                .filter(|r| r.ahead > 0)
                .and_then(|r| r.upstream.clone());
            let base = match base {
                Some(b) => Some(b),
                None => {
                    let git = rv.git.clone();
                    bg(move || {
                        let n: usize = git
                            .run(&["rev-list", "--count", "--first-parent", "HEAD"])
                            .ok()
                            .and_then(|s| s.trim().parse().ok())
                            .unwrap_or(0);
                        if n > 10 {
                            Some("HEAD~10".to_string())
                        } else {
                            None
                        }
                    })
                    .await
                }
            };
            super::rebase::show(rv, base).await;
        }
        "rebase-interactive" => super::rebase::show(rv, Some(arg)).await,
        "clean" => {
            let form = Form::new(&gettext("Remove Untracked Files"), &gettext("Remove"));
            form.description(&gettext("Permanently deletes files that are not tracked by Git."));
            let dirs = form.switch(&gettext("Include untracked directories"), "", true);
            let ignored = form.switch(&gettext("Also remove ignored files"), &gettext("e.g. build output"), false);
            form.ok.add_css_class("destructive-action");
            form.focus_cancel();
            if form.run(&parent).await {
                let mut c = s(&["clean", "-f"]);
                if dirs.is_active() {
                    c.push("-d".into());
                }
                if ignored.is_active() {
                    c.push("-x".into());
                }
                rv.run_ops(&gettext("Clean"), vec![c], OpOptions::default()).await;
            }
        }
        "gc" => {
            rv.run_ops(&gettext("Garbage Collect"), vec![s(&["gc", "--progress"])], net()).await;
        }
        "op-continue" | "op-abort" | "op-skip" => op_control(rv, name).await,
        "checkout-ref" => checkout_ref(rv, &arg).await,
        "checkout-commit" => checkout_commit(rv, &arg).await,
        "rebase-onto" => {
            let cur = rv.snapshot().current_branch().unwrap_or("HEAD").to_string();
            let short = if git::looks_like_oid(&arg) { &arg[..arg.len().min(10)] } else { &arg };
            if confirm(
                &parent,
                &gettext("Rebase"),
                &gettext_f(
                    "Rebase “{branch}” onto “{onto}”?\n\nThis rewrites the commits of {branch}. Don't rebase commits that have already been pushed and shared.",
                    &[("branch", &cur), ("onto", short)],
                ),
                &gettext("Rebase"),
                false,
            )
            .await
            {
                rv.run_ops(&gettext("Rebase"), vec![s(&["rebase", &arg])], OpOptions::default()).await;
            }
        }
        "delete-branch" => delete_branch(rv, &arg).await,
        "delete-remote-branch" => {
            let (remote, branch) = arg.split_once('/').unwrap_or(("origin", &arg));
            if confirm(&parent, &gettext("Delete Remote Branch?"), &gettext_f("Delete “{branch}” from {remote}?", &[("branch", branch), ("remote", remote)]), &gettext("Delete"), true).await {
                rv.run_ops(&gettext("Delete Remote Branch"), vec![s(&["push", "--progress", remote, "--delete", branch])], net()).await;
            }
        }
        "rename-branch" => {
            if let Some(n) = ask_text(&parent, &gettext("Rename Branch"), &gettext_f("New name for “{branch}”", &[("branch", &arg)]), &arg, &gettext("Rename")).await
                && n != arg {
                    rv.run_ops(&gettext("Rename Branch"), vec![s(&["branch", "-m", &arg, &n])], OpOptions::default()).await;
                }
        }
        "track" => {
            let snap = rv.snapshot();
            let mut remotes: Vec<String> = vec![gettext("(none)")];
            remotes.extend(snap.refs.remotes().map(|r| r.name.clone()));
            let cur = snap.refs.find_local(&arg).and_then(|r| r.upstream.clone());
            let form = Form::new(&gettext("Track Remote Branch"), &gettext("OK"));
            form.description(&gettext_f("Choose the remote branch “{branch}” should track.", &[("branch", &arg)]));
            let c = form.combo(&gettext("Remote branch"), &remotes, cur.as_deref());
            form.focus(&c);
            if form.run(&parent).await {
                let v = combo_value(&c);
                let cmd = if c.selected() == 0 {
                    s(&["branch", "--unset-upstream", &arg])
                } else {
                    s(&["branch", &format!("--set-upstream-to={v}"), &arg])
                };
                rv.run_ops(&gettext("Track"), vec![cmd], OpOptions::default()).await;
            }
        }
        "diff-ref" => {
            let git = rv.git.clone();
            let a = arg.clone();
            let r = bg(move || Ok::<_, git::GitError>((git::log::commit(&git, "HEAD")?, git::log::commit(&git, &a)?))).await;
            match r {
                Ok((head, other)) => range_window(rv, &format!("HEAD ↔ {arg}"), head, other),
                Err(e) => show_error(&parent, &gettext("Could not compare"), &e.to_string()),
            }
        }
        "copy-text" => {
            copy_to_clipboard(&arg);
            rv.toast(&gettext("Copied to clipboard"));
        }
        "copy-message" => {
            let git = rv.git.clone();
            if let Ok(m) = bg(move || git::log::message(&git, &arg)).await {
                copy_to_clipboard(m.trim());
                rv.toast(&gettext("Commit message copied"));
            }
        }
        "reset-to" => reset_dialog(rv, &arg).await,
        "revert" => {
            let short = &arg[..arg.len().min(10)];
            let form = Form::new(&gettext("Reverse Commit"), &gettext("Reverse"));
            form.description(&gettext_f("Create a new commit that undoes the changes of {commit}.", &[("commit", short)]));
            let commit_now = form.switch(&gettext("Commit immediately"), "", true);
            form.focus_ok();
            if form.run(&parent).await {
                let git = rv.git.clone();
                let a2 = arg.clone();
                let is_merge = bg(move || git::log::commit(&git, &a2).map(|c| c.parents.len() > 1).unwrap_or(false)).await;
                let mut c = s(&["revert", "--no-edit"]);
                if !commit_now.is_active() {
                    c.push("--no-commit".into());
                }
                if is_merge {
                    c.push("-m".into());
                    c.push("1".into());
                }
                c.push(arg);
                rv.run_ops(&gettext("Reverse Commit"), vec![c], OpOptions::default()).await;
            }
        }
        "cherry-pick" => {
            let oids: Vec<&str> = arg.split_whitespace().collect();
            let form = Form::new(&gettext("Cherry Pick"), &gettext("Cherry Pick"));
            form.description(&if oids.len() == 1 {
                gettext_f("Apply commit {commit} onto the current branch.", &[("commit", &oids[0][..oids[0].len().min(10)])])
            } else {
                ngettext_f(
                    "Apply {n} commit onto the current branch.",
                    "Apply {n} commits onto the current branch.",
                    oids.len() as u32,
                    &[],
                )
            });
            let commit_now = form.switch(&gettext("Commit immediately"), "", true);
            let x = form.switch(&gettext("Append “cherry picked from” line"), "", false);
            form.focus_ok();
            if form.run(&parent).await {
                let mut c = s(&["cherry-pick"]);
                if !commit_now.is_active() {
                    c.push("--no-commit".into());
                }
                if x.is_active() {
                    c.push("-x".into());
                }
                c.extend(oids.iter().map(|o| o.to_string()));
                rv.run_ops(&gettext("Cherry Pick"), vec![c], OpOptions::default()).await;
            }
        }
        "archive" => {
            let fd = gtk::FileDialog::builder()
                .title(gettext("Archive"))
                .initial_name(format!("{}-{}.zip", rv.name, &arg[..arg.len().min(7)]))
                .build();
            let win = parent.root().and_downcast::<gtk::Window>();
            if let Ok(f) = fd.save_future(win.as_ref()).await
                && let Some(p) = f.path() {
                    let ps = p.to_string_lossy().to_string();
                    let fmt = if ps.ends_with(".tar.gz") || ps.ends_with(".tgz") {
                        "tar.gz"
                    } else if ps.ends_with(".tar") {
                        "tar"
                    } else {
                        "zip"
                    };
                    rv.run_ops(&gettext("Archive"), vec![s(&["archive", &format!("--format={fmt}"), "-o", &ps, &arg])], OpOptions::default()).await;
                    rv.toast(&gettext("Archive created"));
                }
        }
        "patch" => {
            let fd = gtk::FileDialog::builder()
                .title(gettext("Create Patch"))
                .initial_name(format!("{}.patch", &arg[..arg.len().min(10)]))
                .build();
            let win = parent.root().and_downcast::<gtk::Window>();
            if let Ok(f) = fd.save_future(win.as_ref()).await
                && let Some(p) = f.path() {
                    let git = rv.git.clone();
                    let a = arg.clone();
                    let r = bg(move || {
                        let out = git.run(&["format-patch", "-1", "--stdout", &a])?;
                        std::fs::write(&p, out).map_err(|e| git::GitError {
                            command: "write patch".into(),
                            stderr: e.to_string(),
                            stdout: String::new(),
                            code: None,
                        })
                    })
                    .await;
                    match r {
                        Ok(()) => rv.toast(&gettext("Patch created")),
                        Err(e) => show_error(&parent, &gettext("Could not create patch"), &e.to_string()),
                    }
                }
        }
        "custom-action" => custom_action(rv, &arg).await,
        n if n.starts_with("file-") => file_action(rv, n, &arg).await,
        "remote-add" => super::repo_settings::edit_remote(rv, None).await,
        "remote-edit" => super::repo_settings::edit_remote(rv, Some(arg)).await,
        "remote-remove" => {
            if confirm(&parent, &gettext("Remove Remote?"), &gettext_f("Remove remote “{remote}” and its remote-tracking branches?", &[("remote", &arg)]), &gettext("Remove"), true).await {
                rv.run_ops(&gettext("Remove Remote"), vec![s(&["remote", "remove", &arg])], OpOptions::default()).await;
            }
        }
        "remote-copy-url" => {
            if let Some(r) = rv.snapshot().remotes.iter().find(|r| r.name == arg) {
                copy_to_clipboard(&r.fetch_url);
                rv.toast(&gettext("URL copied"));
            }
        }
        "open-remote" => {
            let snap = rv.snapshot();
            let name = if arg.is_empty() { snap.default_remote().unwrap_or_default() } else { arg };
            match snap.remotes.iter().find(|r| r.name == name).and_then(|r| web_url(&r.fetch_url)) {
                Some(url) => {
                    let l = gtk::UriLauncher::new(&url);
                    l.launch(parent.root().and_downcast_ref::<gtk::Window>(), None::<&gio::Cancellable>, |_| {});
                }
                None => rv.toast(&gettext("No web URL for this remote")),
            }
        }
        // Development aids used by scripts/screenshot.sh.
        "debug-select-unstaged" => rv.file_status.debug_select(false, arg.parse().unwrap_or(0)),
        "debug-select-staged" => rv.file_status.debug_select(true, arg.parse().unwrap_or(0)),
        "debug-select-commit" => rv.history.debug_select(arg.parse().unwrap_or(0)),
        "debug-select-lines" => rv.file_status.diff.debug_select_lines(&arg),
        "debug-search" => {
            rv.show_view(View::Search);
            rv.search.debug_search(&arg);
        }
        _ => eprintln!("gitree: unknown action {name}"),
    }
}

/// Converts a clone URL to a browsable https URL.
pub fn web_url(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches(".git");
    if let Some(rest) = u.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{host}/{path}"));
    }
    if let Some(rest) = u.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next().unwrap_or(host);
        return Some(format!("https://{host}/{path}"));
    }
    if u.starts_with("http://") || u.starts_with("https://") {
        // Strip credentials.
        if let Some((scheme, rest)) = u.split_once("://") {
            let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
            return Some(format!("{scheme}://{rest}"));
        }
    }
    None
}

// ---------------------------------------------------------------------------

async fn pull_dialog(rv: &Rc<RepoView>, preselect: Option<String>) {
    let snap = rv.snapshot();
    if snap.remotes.is_empty() {
        show_error(&rv.widget, &gettext("No remotes"), &gettext("This repository has no remotes. Add one in Repository Settings."));
        return;
    }
    let Some(current) = snap.current_branch().map(String::from) else {
        show_error(&rv.widget, &gettext("Detached HEAD"), &gettext("Check out a branch before pulling."));
        return;
    };
    let remotes: Vec<String> = snap.remotes.iter().map(|r| r.name.clone()).collect();
    let (pre_remote, pre_branch) = match &preselect {
        Some(p) => {
            let (r, b) = p.split_once('/').unwrap_or(("", p));
            (Some(r.to_string()), Some(b.to_string()))
        }
        None => {
            let up = snap.refs.current().and_then(|r| r.upstream.clone());
            match up.as_deref().and_then(|u| u.split_once('/')) {
                Some((r, b)) => (Some(r.to_string()), Some(b.to_string())),
                None => (snap.default_remote(), Some(current.clone())),
            }
        }
    };

    let form = Form::new(&gettext("Pull"), &gettext("Pull"));
    form.group("");
    let remote = form.combo(&gettext("Pull from repository"), &remotes, pre_remote.as_deref());
    let branches_for = {
        let snap = snap.clone();
        move |r: &str| -> Vec<String> {
            snap.refs
                .remotes()
                .filter(|x| x.remote() == Some(r))
                .filter_map(|x| x.remote_branch().map(String::from))
                .collect()
        }
    };
    let rb = branches_for(&combo_value(&remote));
    let branch = form.combo(&gettext("Remote branch to pull"), &rb, pre_branch.as_deref());
    form.info(&gettext("Pull into local branch"), &current);
    let bf = branches_for.clone();
    let b2 = branch.clone();
    remote.connect_selected_notify(move |c| {
        let list = bf(&combo_value(c));
        let refs: Vec<&str> = list.iter().map(|s| s.as_str()).collect();
        b2.set_model(Some(&gtk::StringList::new(&refs)));
    });
    form.group(&gettext("Options"));
    let commit_now = form.switch(&gettext("Commit merged changes immediately"), "", true);
    let log = form.switch(&gettext("Include messages from commits being merged in merge commit"), "", false);
    let no_ff = form.switch(&gettext("Create new commit even if fast-forward merge"), "", false);
    let rebase = form.switch(&gettext("Rebase instead of merge"), &gettext("Warning: make sure you haven't pushed your changes"), false);
    let b3 = branch.clone();
    form.validate(move || b3.selected_item().is_some());
    form.watch_combo(&branch);
    form.focus_ok();
    if !form.run(&rv.widget).await {
        return;
    }
    let mut c = s(&["pull", "--progress"]);
    if rebase.is_active() {
        c.push("--rebase".into());
    } else {
        c.push("--no-rebase".into());
        if !commit_now.is_active() {
            c.push("--no-commit".into());
        }
        if no_ff.is_active() {
            c.push("--no-ff".into());
        }
        if log.is_active() {
            c.push("--log".into());
        }
    }
    c.push(combo_value(&remote));
    c.push(combo_value(&branch));
    rv.run_ops(&gettext("Pull"), vec![c], net()).await;
}

async fn push_dialog(rv: &Rc<RepoView>, preselect: Option<String>) {
    let snap = rv.snapshot();
    if snap.remotes.is_empty() {
        show_error(&rv.widget, &gettext("No remotes"), &gettext("This repository has no remotes. Add one in Repository Settings."));
        return;
    }
    let remotes: Vec<String> = snap.remotes.iter().map(|r| r.name.clone()).collect();
    let form = Form::new(&gettext("Push"), &gettext("Push"));
    let remote = form.combo(&gettext("Push to repository"), &remotes, snap.default_remote().as_deref());
    form.group(&gettext("Branches to push"));
    let mut rows: Vec<(String, gtk::CheckButton, gtk::Entry, gtk::CheckButton, bool)> = Vec::new();
    let pre = preselect.clone().or_else(|| snap.current_branch().map(String::from));
    for b in snap.refs.locals() {
        let row = adw::ActionRow::builder().title(&b.name).build();
        let check = gtk::CheckButton::new();
        check.set_active(pre.as_deref() == Some(b.name.as_str()));
        row.add_prefix(&check);
        row.set_activatable_widget(Some(&check));
        let remote_name = b
            .upstream
            .as_deref()
            .and_then(|u| u.split_once('/'))
            .map(|(_, br)| br.to_string())
            .unwrap_or_else(|| b.name.clone());
        let entry = gtk::Entry::builder()
            .text(&remote_name)
            .valign(gtk::Align::Center)
            .width_chars(16)
            .tooltip_text(gettext("Remote branch name"))
            .build();
        row.add_suffix(&entry);
        let track = gtk::CheckButton::with_label(&gettext("Track"));
        track.set_active(b.upstream.is_none());
        track.set_tooltip_text(Some(&gettext("Set as upstream (-u)")));
        row.add_suffix(&track);
        if b.ahead > 0 {
            row.set_subtitle(&ngettext_f("{n} commit ahead", "{n} commits ahead", b.ahead, &[]));
        } else if b.upstream.is_none() {
            row.set_subtitle(&gettext("Not yet pushed"));
        }
        form.add(&row);
        rows.push((b.name.clone(), check, entry, track, b.upstream.is_some()));
    }
    form.group(&gettext("Options"));
    let tags = form.switch(&gettext("Push all tags"), "", false);
    let force = form.switch(&gettext("Force push (with lease)"), &gettext("Overwrites remote commits; use with care"), false);
    let checks: Vec<gtk::CheckButton> = rows.iter().map(|r| r.1.clone()).collect();
    for c in &checks {
        let f = Rc::downgrade(&form);
        c.connect_toggled(move |_| {
            if let Some(f) = f.upgrade() {
                f.revalidate();
            }
        });
    }
    let t2 = tags.clone();
    form.validate(move || checks.iter().any(|c| c.is_active()) || t2.is_active());
    let f = Rc::downgrade(&form);
    tags.connect_active_notify(move |_| {
        if let Some(f) = f.upgrade() {
            f.revalidate();
        }
    });
    form.focus_ok();
    if !form.run(&rv.widget).await {
        return;
    }
    let r = combo_value(&remote);
    let mut cmds = Vec::new();
    let mut with_track: Vec<String> = Vec::new();
    let mut without: Vec<String> = Vec::new();
    for (name, check, entry, track, _) in &rows {
        if !check.is_active() {
            continue;
        }
        let rn = entry.text().trim().to_string();
        let spec = format!("refs/heads/{name}:refs/heads/{}", if rn.is_empty() { name } else { &rn });
        if track.is_active() {
            with_track.push(spec);
        } else {
            without.push(spec);
        }
    }
    let base = |track: bool| {
        let mut c = s(&["push", "--progress"]);
        if track {
            c.push("-u".into());
        }
        if force.is_active() {
            c.push("--force-with-lease".into());
        }
        c.push(r.clone());
        c
    };
    if !with_track.is_empty() {
        let mut c = base(true);
        c.extend(with_track);
        cmds.push(c);
    }
    if !without.is_empty() {
        let mut c = base(false);
        c.extend(without);
        cmds.push(c);
    }
    if tags.is_active() {
        cmds.push(s(&["push", "--progress", &r, "--tags"]));
    }
    rv.run_ops(&gettext("Push"), cmds, net()).await;
}

async fn fetch_dialog(rv: &Rc<RepoView>) {
    let snap = rv.snapshot();
    if snap.remotes.is_empty() {
        show_error(&rv.widget, &gettext("No remotes"), &gettext("This repository has no remotes."));
        return;
    }
    let form = Form::new(&gettext("Fetch"), &gettext("OK"));
    let all = form.switch(&gettext("Fetch from all remotes"), "", true);
    let prune = form.switch(&gettext("Prune tracking branches no longer present on remote(s)"), "", true);
    let tags = form.switch(&gettext("Fetch and store all tags locally"), "", true);
    form.focus_ok();
    if !form.run(&rv.widget).await {
        return;
    }
    let mut c = s(&["fetch", "--progress"]);
    if all.is_active() {
        c.push("--all".into());
    }
    if prune.is_active() {
        c.push("--prune".into());
    }
    if tags.is_active() {
        c.push("--tags".into());
    }
    if !all.is_active()
        && let Some(r) = snap.default_remote() {
            c.push(r);
        }
    rv.run_ops(&gettext("Fetch"), vec![c], net()).await;
}

fn valid_ref_name(git: &Git, name: &str) -> bool {
    !name.trim().is_empty() && git.check(&["check-ref-format", "--branch", name.trim()])
}

async fn branch_dialog(rv: &Rc<RepoView>, at: &str) {
    let snap = rv.snapshot();
    let form = Form::new(&gettext("New Branch"), &gettext("Create Branch"));
    let current = snap.current_branch().map(String::from).unwrap_or_else(|| gettext("detached HEAD"));
    form.info(&gettext("Current branch"), &current);
    let name = form.entry(&gettext("New branch name"), "");
    let from = if at.is_empty() {
        form.info(&pgettext("noun", "Commit"), &gettext("Working copy parent (HEAD)"));
        None
    } else {
        form.info(&pgettext("noun", "Commit"), &gettext_f("Specified commit: {commit}", &[("commit", &at[..at.len().min(10)])]));
        Some(at.to_string())
    };
    let checkout = form.switch(&gettext("Checkout new branch"), "", true);
    let git = rv.git.clone();
    let existing: Vec<String> = snap.refs.locals().map(|r| r.name.clone()).collect();
    let n2 = name.clone();
    form.watch(&name);
    form.validate(move || {
        let t = n2.text().trim().to_string();
        valid_ref_name(&git, &t) && !existing.contains(&t)
    });
    form.focus(&name);
    if !form.run(&rv.widget).await {
        return;
    }
    let n = name.text().trim().to_string();
    let mut c = if checkout.is_active() {
        s(&["checkout", "-b", &n])
    } else {
        s(&["branch", &n])
    };
    if let Some(f) = from {
        c.push(f);
    }
    rv.run_ops(&gettext("New Branch"), vec![c], OpOptions::default()).await;
}

async fn merge_dialog(rv: &Rc<RepoView>, preselect: Option<String>) {
    let snap = rv.snapshot();
    let current = snap.current_branch().unwrap_or("HEAD").to_string();
    let mut candidates: Vec<String> = snap
        .refs
        .locals()
        .filter(|r| !r.is_head)
        .map(|r| r.name.clone())
        .collect();
    candidates.extend(snap.refs.remotes().map(|r| r.name.clone()));
    if candidates.is_empty() {
        show_error(&rv.widget, &gettext("Nothing to merge"), &gettext("There are no other branches."));
        return;
    }
    let form = Form::new(&gettext("Merge"), &gettext("Merge"));
    form.info(&gettext("Merge into"), &current);
    let from = form.combo(&gettext("Branch to merge"), &candidates, preselect.as_deref());
    form.group(&gettext("Options"));
    let commit_now = form.switch(&gettext("Commit merged changes immediately"), "", true);
    let log = form.switch(&gettext("Include messages from commits being merged in merge commit"), "", false);
    let no_ff = form.switch(&gettext("Create a new commit even if fast-forward is possible"), "", false);
    let squash = form.switch(&gettext("Squash (merge changes as a single commit)"), "", false);
    form.focus(&from);
    if !form.run(&rv.widget).await {
        return;
    }
    let mut c = s(&["merge", "--no-edit"]);
    if squash.is_active() {
        c.push("--squash".into());
    } else {
        if !commit_now.is_active() {
            c.push("--no-commit".into());
        }
        if no_ff.is_active() {
            c.push("--no-ff".into());
        }
        if log.is_active() {
            c.push("--log".into());
        }
    }
    c.push(combo_value(&from));
    rv.run_ops(&gettext("Merge"), vec![c], OpOptions::default()).await;
}

async fn stash_dialog(rv: &Rc<RepoView>) {
    if !rv.snapshot().has_changes() {
        rv.toast(&gettext("There are no local changes to stash"));
        return;
    }
    let form = Form::new(&gettext("Stash"), &gettext("Stash"));
    form.description(&gettext("Stash all local changes so you can reapply them later."));
    let msg = form.entry(&gettext("Message"), "");
    let keep = form.switch(&gettext("Keep staged changes"), "", false);
    let untracked = form.switch(&gettext("Include untracked files"), "", true);
    form.focus(&msg);
    if !form.run(&rv.widget).await {
        return;
    }
    let mut c = s(&["stash", "push"]);
    if !msg.text().trim().is_empty() {
        c.push("-m".into());
        c.push(msg.text().trim().to_string());
    }
    if keep.is_active() {
        c.push("--keep-index".into());
    }
    if untracked.is_active() {
        c.push("--include-untracked".into());
    }
    rv.run_ops(&gettext("Stash"), vec![c], OpOptions::default()).await;
}

async fn discard_dialog(rv: &Rc<RepoView>) {
    let snap = rv.snapshot();
    if !snap.has_changes() {
        rv.toast(&gettext("There are no local changes to discard"));
        return;
    }
    let form = Form::new(&gettext("Discard Changes"), &gettext("Discard"));
    form.description(&gettext("Discard local changes. This cannot be undone."));
    let tracked = form.switch(&gettext("Reset all tracked files to HEAD"), &gettext("Staged and unstaged changes are lost"), true);
    let untracked = form.switch(&gettext("Remove untracked files"), "", false);
    form.ok.add_css_class("destructive-action");
    let (t2, u2) = (tracked.clone(), untracked.clone());
    form.validate(move || t2.is_active() || u2.is_active());
    let f = Rc::downgrade(&form);
    tracked.connect_active_notify(move |_| {
        if let Some(f) = f.upgrade() {
            f.revalidate();
        }
    });
    let f = Rc::downgrade(&form);
    untracked.connect_active_notify(move |_| {
        if let Some(f) = f.upgrade() {
            f.revalidate();
        }
    });
    form.focus_cancel();
    if !form.run(&rv.widget).await {
        return;
    }
    let mut cmds = Vec::new();
    if tracked.is_active() {
        cmds.push(s(&["reset", "--hard", "HEAD"]));
    }
    if untracked.is_active() {
        cmds.push(s(&["clean", "-fd"]));
    }
    rv.run_ops(&gettext("Discard"), cmds, OpOptions::default()).await;
}

async fn tag_dialog(rv: &Rc<RepoView>, at: &str) {
    let snap = rv.snapshot();
    let form = Form::new(&gettext("Add Tag"), &gettext("Add"));
    let name = form.entry(&gettext("Tag name"), "");
    let target = if at.is_empty() {
        form.info(&pgettext("noun", "Commit"), &gettext("Working copy parent (HEAD)"));
        "HEAD".to_string()
    } else {
        form.info(&pgettext("noun", "Commit"), &gettext_f("Specified commit: {commit}", &[("commit", &at[..at.len().min(10)])]));
        at.to_string()
    };
    let remotes: Vec<String> = snap.remotes.iter().map(|r| r.name.clone()).collect();
    let push = form.switch(&gettext("Push tag"), "", false);
    let remote = form.combo(&gettext("Remote"), &remotes, snap.default_remote().as_deref());
    push.set_visible(!remotes.is_empty());
    remote.set_visible(!remotes.is_empty());
    form.group(&gettext("Advanced"));
    let light = form.switch(&gettext("Lightweight tag (no message)"), "", false);
    let sign = form.switch(&gettext("Sign tag (GPG)"), "", false);
    let force = form.switch(&gettext("Move existing tag (force)"), "", false);
    let msg = form.text(&gettext("Message"), "", 60);
    let git = rv.git.clone();
    let n2 = name.clone();
    form.watch(&name);
    form.validate(move || {
        let t = n2.text().trim().to_string();
        !t.is_empty() && git.check(&["check-ref-format", &format!("refs/tags/{t}")])
    });
    form.focus(&name);
    if !form.run(&rv.widget).await {
        return;
    }
    let n = name.text().trim().to_string();
    let mut c = s(&["tag"]);
    if force.is_active() {
        c.push("-f".into());
    }
    if !light.is_active() {
        if sign.is_active() {
            c.push("-s".into());
        } else {
            c.push("-a".into());
        }
        let m = text_of(&msg);
        c.push("-m".into());
        c.push(if m.trim().is_empty() { n.clone() } else { m });
    }
    c.push(n.clone());
    c.push(target);
    let mut cmds = vec![c];
    if push.is_active() {
        let mut p = s(&["push", "--progress"]);
        if force.is_active() {
            p.push("--force".into());
        }
        p.push(combo_value(&remote));
        p.push(format!("refs/tags/{n}"));
        cmds.push(p);
    }
    rv.run_ops(&gettext("Tag"), cmds, OpOptions { network: push.is_active(), ..Default::default() }).await;
}

async fn reset_dialog(rv: &Rc<RepoView>, oid: &str) {
    let cur = rv.snapshot().current_branch().unwrap_or("HEAD").to_string();
    let form = Form::new(&gettext("Reset to Commit"), &gettext("Reset"));
    form.description(&gettext_f("Reset “{branch}” to commit {commit}.", &[("branch", &cur), ("commit", &oid[..oid.len().min(10)])]));
    let modes = vec![
        gettext("Soft — keep all local changes (staged)"),
        gettext("Mixed — keep working copy but reset index"),
        gettext("Hard — discard all working copy changes"),
    ];
    let mode = form.combo(&gettext("Using mode"), &modes, Some(&modes[1]));
    form.focus(&mode);
    if !form.run(&rv.widget).await {
        return;
    }
    let flag = match mode.selected() {
        0 => "--soft",
        2 => "--hard",
        _ => "--mixed",
    };
    if flag == "--hard"
        && config::with(|s| s.confirm_dangerous)
        && !confirm(&rv.widget, &gettext("Hard Reset?"), &gettext("All uncommitted changes will be permanently lost."), &gettext("Reset"), true).await
    {
        return;
    }
    rv.run_ops(&gettext("Reset"), vec![s(&["reset", flag, oid])], OpOptions::default()).await;
}

async fn delete_branch(rv: &Rc<RepoView>, name: &str) {
    let snap = rv.snapshot();
    let upstream = snap.refs.find_local(name).and_then(|r| r.upstream.clone());
    let form = Form::new(&gettext("Delete Branch"), &gettext("Delete"));
    form.description(&gettext_f("Delete local branch “{branch}”?", &[("branch", name)]));
    let force = form.switch(&gettext("Force delete regardless of merge status"), "", false);
    let remote = form.switch(
        &match &upstream {
            Some(u) => gettext_f("Also delete the remote branch ({branch})", &[("branch", u)]),
            None => gettext("Also delete the remote branch"),
        },
        "",
        false,
    );
    remote.set_visible(upstream.is_some());
    form.ok.add_css_class("destructive-action");
    form.focus_cancel();
    if !form.run(&rv.widget).await {
        return;
    }
    let mut cmds = vec![s(&["branch", if force.is_active() { "-D" } else { "-d" }, name])];
    if remote.is_active()
        && let Some((r, b)) = upstream.as_deref().and_then(|u| u.split_once('/')) {
            cmds.push(s(&["push", "--progress", r, "--delete", b]));
        }
    rv.run_ops(&gettext("Delete Branch"), cmds, OpOptions { network: remote.is_active(), ..Default::default() }).await;
}

async fn checkout_ref(rv: &Rc<RepoView>, full: &str) {
    let snap = rv.snapshot();
    if let Some(name) = full.strip_prefix("refs/heads/") {
        rv.run_ops(&gettext("Checkout"), vec![s(&["checkout", name])], OpOptions::default()).await;
        return;
    }
    let Some(remote_ref) = full.strip_prefix("refs/remotes/") else { return };
    let (_, branch) = remote_ref.split_once('/').unwrap_or(("", remote_ref));
    // A local branch already tracking it?
    if let Some(local) = snap.refs.locals().find(|l| l.upstream.as_deref() == Some(remote_ref)) {
        let form = Form::new(&gettext("Checkout"), &gettext("Checkout"));
        form.description(&gettext_f("Local branch “{branch}” already tracks {upstream}.", &[("branch", &local.name), ("upstream", remote_ref)]));
        let pull = form.switch(&gettext("Pull after checkout (fast-forward only)"), "", true);
        form.focus_ok();
        if form.run(&rv.widget).await {
            let mut cmds = vec![s(&["checkout", &local.name])];
            if pull.is_active() {
                cmds.push(s(&["merge", "--ff-only", remote_ref]));
            }
            rv.run_ops(&gettext("Checkout"), cmds, OpOptions::default()).await;
        }
        return;
    }
    let form = Form::new(&gettext("Checkout Remote Branch"), &gettext("Checkout"));
    form.info(&gettext("Checkout remote branch"), remote_ref);
    let name = form.entry(&gettext("New local branch name"), branch);
    let track = form.switch(&gettext("Local branch should track remote branch"), "", true);
    let git = rv.git.clone();
    let existing: Vec<String> = snap.refs.locals().map(|r| r.name.clone()).collect();
    let n2 = name.clone();
    form.watch(&name);
    form.validate(move || {
        let t = n2.text().trim().to_string();
        valid_ref_name(&git, &t) && !existing.contains(&t)
    });
    form.focus(&name);
    if form.run(&rv.widget).await {
        let n = name.text().trim().to_string();
        let c = s(&["checkout", "-b", &n, if track.is_active() { "--track" } else { "--no-track" }, remote_ref]);
        rv.run_ops(&gettext("Checkout"), vec![c], OpOptions::default()).await;
    }
}

async fn checkout_commit(rv: &Rc<RepoView>, oid: &str) {
    let snap = rv.snapshot();
    let locals: Vec<String> = snap
        .refs
        .locals()
        .filter(|r| r.oid == oid && !r.is_head)
        .map(|r| r.name.clone())
        .collect();
    let form = Form::new(&gettext("Checkout"), &gettext("Checkout"));
    let mut options: Vec<String> = locals.iter().map(|l| gettext_f("Branch {branch}", &[("branch", l)])).collect();
    options.push(gettext_f("Detached HEAD at {commit}", &[("commit", &oid[..oid.len().min(10)])]));
    let choice = form.combo(&gettext("Checkout"), &options, None);
    form.description(&if locals.is_empty() {
        gettext("You will be in “detached HEAD” state: commits made here won't belong to any branch unless you create one.")
    } else {
        gettext("Checking out a branch is recommended over a detached HEAD.")
    });
    let clean = form.switch(&gettext("Discard local changes"), "", false);
    form.focus(&choice);
    if !form.run(&rv.widget).await {
        return;
    }
    let sel = choice.selected() as usize;
    let mut c = s(&["checkout"]);
    if clean.is_active() {
        c.push("-f".into());
    }
    if sel < locals.len() {
        c.push(locals[sel].clone());
    } else {
        c.push("--detach".into());
        c.push(oid.to_string());
    }
    rv.run_ops(&gettext("Checkout"), vec![c], OpOptions::default()).await;
}

async fn op_control(rv: &Rc<RepoView>, name: &str) {
    let snap = rv.snapshot();
    let Some(cmd) = snap.op.command() else { return };
    match name {
        "op-continue" => {
            if snap.status.has_conflicts() {
                show_error(&rv.widget, &gettext("Unresolved conflicts"), &gettext("Resolve the conflicted files and mark them resolved first."));
                return;
            }
            let c = match snap.op {
                OpState::Merge => s(&["commit", "--no-edit"]),
                OpState::Bisect => return,
                _ => s(&[cmd, "--continue"]),
            };
            rv.run_ops(&gettext("Continue"), vec![c], OpOptions::default()).await;
        }
        "op-skip" => {
            rv.run_ops(&gettext("Skip"), vec![s(&[cmd, "--skip"])], OpOptions::default()).await;
        }
        _ => {
            if !confirm(&rv.widget, &gettext("Abort?"), &gettext_f(
                // Translators: {operation} is a git command name such as "merge" or "rebase".
                "Abort the {operation} in progress and restore the previous state?",
                &[("operation", cmd)],
            ), &gettext("Abort"), true).await {
                return;
            }
            let c = match snap.op {
                OpState::Bisect => s(&["bisect", "reset"]),
                _ => s(&[cmd, "--abort"]),
            };
            rv.run_ops(&gettext("Abort"), vec![c], OpOptions::default()).await;
        }
    }
}

async fn subtree_dialog(rv: &Rc<RepoView>) {
    let form = Form::new(&gettext("Add / Link Subtree"), &gettext("OK"));
    let url = form.entry(&gettext("Source URL"), "");
    let branch = form.entry(&gettext("Branch or commit"), "main");
    let prefix = form.entry(&gettext("Local relative path"), "");
    let squash = form.switch(&gettext("Squash commits"), "", true);
    let link_only = form.switch(&gettext("Only link an existing subtree folder"), "", false);
    let (u2, p2) = (url.clone(), prefix.clone());
    form.watch(&url);
    form.watch(&prefix);
    form.validate(move || !u2.text().trim().is_empty() && !p2.text().trim().is_empty());
    form.focus(&url);
    if !form.run(&rv.widget).await {
        return;
    }
    let p = prefix.text().trim().trim_end_matches('/').to_string();
    let u = url.text().trim().to_string();
    let b = branch.text().trim().to_string();
    let mut cmds = Vec::new();
    if !link_only.is_active() {
        let mut c = s(&["subtree", "add", &format!("--prefix={p}"), &u, &b]);
        if squash.is_active() {
            c.push("--squash".into());
        }
        cmds.push(c);
    }
    cmds.push(s(&["config", &format!("gitree.subtree.{p}.url"), &u]));
    cmds.push(s(&["config", &format!("gitree.subtree.{p}.branch"), &b]));
    rv.run_ops(&gettext("Subtree"), cmds, net()).await;
}

async fn apply_patch_dialog(rv: &Rc<RepoView>) {
    let fd = gtk::FileDialog::builder().title(gettext("Choose a Patch File")).build();
    let win = rv.widget.root().and_downcast::<gtk::Window>();
    let Ok(f) = fd.open_future(win.as_ref()).await else { return };
    let Some(p) = f.path() else { return };
    let form = Form::new(&gettext("Apply Patch"), &gettext("Apply"));
    form.info(&gettext("Patch"), &p.to_string_lossy());
    let modes = vec![
        gettext("Modify working copy files"),
        gettext("Modify working copy and index (stage)"),
        gettext("Import as commits (git am)"),
    ];
    let mode = form.combo(&gettext("Apply to"), &modes, None);
    form.focus(&mode);
    if !form.run(&rv.widget).await {
        return;
    }
    let ps = p.to_string_lossy().to_string();
    let c = match mode.selected() {
        0 => s(&["apply", &ps]),
        1 => s(&["apply", "--index", &ps]),
        _ => s(&["am", "--3way", &ps]),
    };
    rv.run_ops(&gettext("Apply Patch"), vec![c], OpOptions::default()).await;
}

fn range_window(rv: &Rc<RepoView>, title: &str, from: git::log::Commit, to: git::log::Commit) {
    let win = adw::Window::builder()
        .title(title)
        .default_width(1000)
        .default_height(700)
        .build();
    if let Some(root) = rv.widget.root().and_downcast::<gtk::Window>() {
        win.set_transient_for(Some(&root));
    }
    let details = super::history::CommitDetails::new(rv.weak(), rv.git.clone(), None);
    details.show_range(&from, &to);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&details.widget));
    win.set_content(Some(&tv));
    win.insert_action_group("repo", Some(&rv.actions));
    win.present();
    let holder = std::cell::RefCell::new(Some(details));
    win.connect_close_request(move |_| {
        holder.borrow_mut().take();
        gtk::glib::Propagation::Proceed
    });
}

/// Launches the configured external diff tool.
pub fn external_diff(rv: &Rc<RepoView>, path: &str, staged: bool, range: Option<(String, String)>) {
    let tool = config::with(|s| s.diff_tool.clone());
    let mut rest = s(&["-y", "--no-prompt"]);
    if staged {
        rest.push("--cached".into());
    }
    if let Some((a, b)) = range {
        rest.push(a);
        rest.push(b);
    }
    rest.push("--".into());
    rest.push(path.to_string());
    let full = tool_invocation("difftool", &tool, rest);
    let git = rv.git.clone();
    let widget = rv.widget.clone();
    spawn(async move {
        let r = bg(move || git.run(&full)).await;
        if let Err(e) = r {
            show_error(&widget, &gettext("External diff failed"), &e.to_string());
        }
    });
}

/// `-c ... --tool=...` arguments for a difftool/mergetool setting that is
/// either a known tool name or a custom command line.
fn tool_config_args(kind: &str, tool: &str) -> Vec<String> {
    let tool = tool.trim();
    if tool.is_empty() {
        return Vec::new();
    }
    if tool.contains(' ') || tool.contains('$') {
        let mut v = s(&["-c", &format!("{kind}.gitree.cmd={tool}")]);
        if kind == "mergetool" {
            v.extend(s(&["-c", "mergetool.gitree.trustExitCode=true"]));
        }
        v.push("--tool=gitree".to_string());
        // --tool must come after the subcommand; caller reorders.
        return v;
    }
    vec![format!("--tool={tool}")]
}

fn tool_invocation(kind: &str, tool: &str, rest: Vec<String>) -> Vec<String> {
    let args = tool_config_args(kind, tool);
    let (cfg, tool_flag): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| !a.starts_with("--tool"));
    let mut v = cfg;
    v.push(kind.to_string());
    v.extend(tool_flag);
    v.extend(rest);
    v
}

async fn custom_action(rv: &Rc<RepoView>, arg: &str) {
    let mut parts = arg.splitn(3, '|');
    let idx: usize = parts.next().and_then(|i| i.parse().ok()).unwrap_or(usize::MAX);
    let sha = parts.next().unwrap_or("").to_string();
    let file = parts.next().unwrap_or("").to_string();
    let Some(action) = config::with(|s| s.custom_actions.get(idx).cloned()) else { return };
    let repo = rv.git.workdir.to_string_lossy().to_string();
    let args: Vec<String> = action
        .args
        .split_whitespace()
        .map(|a| a.replace("$REPO", &repo).replace("$SHA", &sha).replace("$FILE", &file))
        .collect();
    let cmd = action.command.clone();
    let wd = rv.git.workdir.clone();
    let show = action.show_output;
    let r = bg(move || {
        let mut c = std::process::Command::new(&cmd);
        c.args(&args).current_dir(wd);
        crate::host::wrap(c, true).output()
    })
    .await;
    match r {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if show || !out.status.success() {
                show_error(&rv.widget, &action.name, &text);
            }
        }
        Err(e) => show_error(&rv.widget, &action.name, &e.to_string()),
    }
    rv.refresh();
}

async fn file_action(rv: &Rc<RepoView>, name: &str, arg: &str) {
    let parent = rv.widget.clone();
    let (side, paths) = decode_target(arg);
    let snap = rv.snapshot();
    let entry = |p: &str| snap.status.entries.iter().find(|e| e.path == p).cloned();
    match name {
        "file-stage" => rv.file_status.stage_paths(paths),
        "file-unstage" => rv.file_status.unstage_paths(paths),
        "file-discard" => {
            let list = paths.join("\n");
            if !confirm(&parent, &gettext("Discard Changes?"), &gettext_f("Discard all changes to:\n{files}\n\nThis cannot be undone.", &[("files", &list)]), &gettext("Discard"), true).await {
                return;
            }
            let mut tracked = Vec::new();
            let mut untracked = Vec::new();
            let mut added = Vec::new();
            for p in &paths {
                match entry(p) {
                    Some(e) if e.untracked => untracked.push(p.clone()),
                    Some(e) if e.index == 'A' => added.push(p.clone()),
                    _ => tracked.push(p.clone()),
                }
            }
            let mut cmds = Vec::new();
            let has_head = snap.refs.head_oid.is_some();
            if !tracked.is_empty() {
                let mut c = if has_head {
                    s(&["checkout", "HEAD", "--"])
                } else {
                    s(&["checkout", "--"])
                };
                c.extend(tracked);
                cmds.push(c);
            }
            if !added.is_empty() {
                let mut c = s(&["rm", "-f", "-q", "--"]);
                c.extend(added);
                cmds.push(c);
            }
            if !untracked.is_empty() {
                let mut c = s(&["clean", "-f", "-d", "-q", "--"]);
                c.extend(untracked);
                cmds.push(c);
            }
            rv.run_ops(&gettext("Discard"), cmds, OpOptions::default()).await;
        }
        "file-remove" => {
            if !confirm(&parent, &gettext("Remove Files?"), &gettext_f("Delete from disk and stage removal:\n{files}", &[("files", &paths.join("\n"))]), &gettext("Remove"), true).await {
                return;
            }
            let (untracked, tracked): (Vec<String>, Vec<String>) =
                paths.into_iter().partition(|p| entry(p).is_some_and(|e| e.untracked));
            for p in &untracked {
                let full = rv.git.workdir.join(p);
                let _ = if full.is_dir() { std::fs::remove_dir_all(full) } else { std::fs::remove_file(full) };
            }
            if !tracked.is_empty() {
                let mut c = s(&["rm", "-f", "-r", "--"]);
                c.extend(tracked);
                rv.run_ops(&gettext("Remove"), vec![c], OpOptions::default()).await;
            } else {
                rv.refresh();
            }
        }
        "file-stop-tracking" => {
            let mut c = s(&["rm", "--cached", "-r", "-q", "--"]);
            c.extend(paths);
            rv.run_ops(&gettext("Stop Tracking"), vec![c], OpOptions::default()).await;
        }
        "file-ignore" => ignore_dialog(rv, &paths).await,
        "file-open" => super::open_file(&rv.git.workdir.join(arg)),
        "file-show" => super::show_in_folder(&rv.git.workdir.join(arg)),
        "file-difftool" => {
            if let Some(p) = paths.first() {
                external_diff(rv, p, side == Side::Staged, None);
            }
        }
        "file-log" => super::history::file_log_window(rv, arg),
        "file-blame" => super::blame::show(rv, arg, None),
        "file-blame-at" => {
            if let Some((rev, p)) = arg.split_once('|') {
                super::blame::show(rv, p, Some(rev.to_string()));
            }
        }
        "file-checkout-at" => {
            if let Some((rev, p)) = arg.split_once('|')
                && confirm(&parent, &gettext("Reset File?"), &gettext_f("Replace “{file}” with its version at {commit}? Local changes to it are lost.", &[("file", p), ("commit", &rev[..rev.len().min(11)])]), &gettext("Reset"), true).await {
                    rv.run_ops(&gettext("Reset File"), vec![s(&["checkout", rev, "--", p])], OpOptions::default()).await;
                }
        }
        "file-resolve-mine" | "file-resolve-theirs" => {
            // During a rebase "ours" is the upstream; map Mine/Theirs to what the user sees.
            let rebasing = matches!(snap.op, OpState::Rebase { .. });
            let mine = name == "file-resolve-mine";
            let which = if mine != rebasing { "--ours" } else { "--theirs" };
            let mut c1 = s(&["checkout", which, "--"]);
            c1.extend(paths.iter().cloned());
            let mut c2 = s(&["add", "--"]);
            c2.extend(paths);
            rv.run_ops(&gettext("Resolve"), vec![c1, c2], OpOptions::default()).await;
        }
        "file-mark-resolved" => {
            let mut c = s(&["add", "--"]);
            c.extend(paths);
            rv.run_ops(&gettext("Mark Resolved"), vec![c], OpOptions::default()).await;
        }
        "file-mark-unresolved" => {
            let mut c = s(&["checkout", "-m", "--"]);
            c.extend(paths);
            rv.run_ops(&gettext("Mark Unresolved"), vec![c], OpOptions::default()).await;
        }
        "file-mergetool" => {
            let tool = config::with(|s| s.merge_tool.clone());
            let mut rest = s(&["-y", "--no-prompt"]);
            rest.push("--".into());
            rest.extend(paths);
            let mut c = s(&["-c", "mergetool.keepBackup=false"]);
            c.extend(tool_invocation("mergetool", &tool, rest));
            rv.run_ops(&gettext("External Merge Tool"), vec![c], OpOptions::default()).await;
        }
        _ => {}
    }
}

async fn ignore_dialog(rv: &Rc<RepoView>, paths: &[String]) {
    let Some(first) = paths.first() else { return };
    let form = Form::new(&gettext("Ignore"), &gettext("OK"));
    let mut options = vec![if paths.len() == 1 {
        gettext_f("Ignore exact filename: {file}", &[("file", first)])
    } else {
        ngettext_f("Ignore {n} exact filename", "Ignore {n} exact filenames", paths.len() as u32, &[])
    }];
    let ext = std::path::Path::new(first)
        .extension()
        .map(|e| e.to_string_lossy().to_string());
    if let Some(e) = &ext {
        options.push(gettext_f("Ignore all files with this extension: {pattern}", &[("pattern", &format!("*.{e}"))]));
    }
    let mut dirs: Vec<String> = Vec::new();
    let mut acc = String::new();
    for part in first.split('/').collect::<Vec<_>>().iter().rev().skip(1).rev() {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(part);
        dirs.push(acc.clone());
    }
    for d in &dirs {
        options.push(gettext_f("Ignore everything beneath: {dir}", &[("dir", &format!("{d}/"))]));
    }
    let what = form.combo(&gettext("Ignore"), &options, None);
    let files = vec![
        gettext(".gitignore (shared with the repository)"),
        gettext(".git/info/exclude (this copy only)"),
    ];
    let into = form.combo(&gettext("Add this ignore entry to"), &files, None);
    form.focus(&what);
    if !form.run(&rv.widget).await {
        return;
    }
    let sel = what.selected() as usize;
    let patterns: Vec<String> = if sel == 0 {
        paths.iter().map(|p| format!("/{p}")).collect()
    } else if ext.is_some() && sel == 1 {
        vec![format!("*.{}", ext.unwrap())]
    } else {
        let di = sel - 1 - usize::from(ext.is_some());
        vec![format!("/{}/", dirs.get(di).cloned().unwrap_or_default())]
    };
    let file: PathBuf = if into.selected() == 0 {
        rv.git.workdir.join(".gitignore")
    } else {
        rv.git_dir.join("info").join("exclude")
    };
    let mut content = std::fs::read_to_string(&file).unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    for p in patterns {
        content.push_str(&p);
        content.push('\n');
    }
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&file, content) {
        show_error(&rv.widget, &gettext("Could not update ignore file"), &e.to_string());
    }
    rv.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_urls() {
        assert_eq!(web_url("git@github.com:a/b.git").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(web_url("https://user@gitlab.com/x/y.git").as_deref(), Some("https://gitlab.com/x/y"));
        assert_eq!(web_url("ssh://git@host:22/p/q.git").as_deref(), Some("https://host/p/q"));
        assert_eq!(web_url("/local/path"), None);
    }

    #[test]
    fn tool_args() {
        assert_eq!(tool_invocation("mergetool", "meld", vec![]), vec!["mergetool", "--tool=meld"]);
        let v = tool_invocation("difftool", "code --diff $LOCAL $REMOTE", vec![]);
        assert_eq!(v[0], "-c");
        assert!(v.contains(&"difftool".to_string()));
        assert_eq!(v.last().unwrap(), "--tool=gitree");
    }
}
