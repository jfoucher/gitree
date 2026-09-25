//! Repository settings (remotes, identity, .gitignore, config) and Git LFS.

use super::form::{text_of, Form};
use super::progress::OpOptions;
use super::repo_view::RepoView;
use super::{bg, show_error, spawn};
use crate::git;
use crate::i18n::{gettext, gettext_f};
use adw::prelude::*;
use std::rc::Rc;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Shows the repository settings dialog (`page`: 0 remotes, 1 advanced).
pub fn show(rv: &Rc<RepoView>, _page: u32) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&gettext("Repository Settings"));
    dialog.set_search_enabled(false);

    // Remotes page
    let remotes_page = adw::PreferencesPage::builder()
        .title(gettext("Remotes"))
        .icon_name("gitree-remote-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Remote repositories"))
        .build();
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.add_css_class("flat");
    add.set_tooltip_text(Some(&gettext("Add remote")));
    group.set_header_suffix(Some(&add));
    for r in &rv.snapshot().remotes {
        let row = adw::ActionRow::builder()
            .title(&r.name)
            .subtitle(&r.fetch_url)
            .subtitle_selectable(true)
            .build();
        let edit = gtk::Button::from_icon_name("document-edit-symbolic");
        edit.add_css_class("flat");
        edit.set_valign(gtk::Align::Center);
        let del = gtk::Button::from_icon_name("user-trash-symbolic");
        del.add_css_class("flat");
        del.set_valign(gtk::Align::Center);
        row.add_suffix(&edit);
        row.add_suffix(&del);
        let (rv2, name, d2) = (rv.clone(), r.name.clone(), dialog.clone());
        edit.connect_clicked(move |_| {
            d2.close();
            super::dialogs::dispatch(&rv2, "remote-edit", name.clone());
        });
        let (rv2, name, d2) = (rv.clone(), r.name.clone(), dialog.clone());
        del.connect_clicked(move |_| {
            d2.close();
            super::dialogs::dispatch(&rv2, "remote-remove", name.clone());
        });
        group.add(&row);
    }
    if rv.snapshot().remotes.is_empty() {
        let row = adw::ActionRow::builder().title(gettext("No remotes")).build();
        row.add_css_class("dim-label");
        group.add(&row);
    }
    let (rv2, d2) = (rv.clone(), dialog.clone());
    add.connect_clicked(move |_| {
        d2.close();
        super::dialogs::dispatch(&rv2, "remote-add", String::new());
    });
    remotes_page.add(&group);
    dialog.add(&remotes_page);

    // Advanced page: identity + files
    let adv = adw::PreferencesPage::builder()
        .title(gettext("Advanced"))
        .icon_name("preferences-system-symbolic")
        .build();
    let ident = adw::PreferencesGroup::builder()
        .title(gettext("User information"))
        .description(gettext("Leave empty to use the global identity"))
        .build();
    let local = |k: &str| {
        rv.git
            .run(&["config", "--local", "--get", k])
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let name = adw::EntryRow::builder().title(gettext("Full name")).text(local("user.name")).build();
    let email = adw::EntryRow::builder().title(gettext("Email address")).text(local("user.email")).build();
    name.set_show_apply_button(true);
    email.set_show_apply_button(true);
    for (row, key) in [(&name, "user.name"), (&email, "user.email")] {
        let git = rv.git.clone();
        let key = key.to_string();
        row.connect_apply(move |r| {
            let v = r.text().trim().to_string();
            let _ = if v.is_empty() {
                git.run(&["config", "--local", "--unset", &key])
            } else {
                git.run(&["config", "--local", &key, &v])
            };
        });
        ident.add(row);
    }
    adv.add(&ident);

    let files = adw::PreferencesGroup::builder().title(gettext("Files")).build();
    let edit_row = |title: &str, sub: &str, path: std::path::PathBuf| {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(sub)
            .activatable(true)
            .build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let d = dialog.clone();
        let title = title.to_string();
        row.connect_activated(move |_| {
            edit_file_page(&d, &title, path.clone());
        });
        row
    };
    files.add(&edit_row(&gettext("Edit .gitignore"), &gettext("Ignore patterns shared with the repository"), rv.git.workdir.join(".gitignore")));
    files.add(&edit_row(&gettext("Edit local excludes"), &gettext(".git/info/exclude — not shared"), rv.git_dir.join("info").join("exclude")));
    files.add(&edit_row(&gettext("Edit config file"), ".git/config", rv.git_dir.join("config")));
    files.add(&edit_row(&gettext("Edit .gitattributes"), &gettext("Line endings, LFS tracking, diff drivers"), rv.git.workdir.join(".gitattributes")));
    adv.add(&files);
    dialog.add(&adv);

    dialog.present(Some(&rv.widget));
    let rv2 = rv.clone();
    dialog.connect_closed(move |_| rv2.refresh());
}

fn edit_file_page(dialog: &adw::PreferencesDialog, title: &str, path: std::path::PathBuf) {
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let tv = gtk::TextView::builder()
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .vexpand(true)
        .build();
    tv.buffer().set_text(&content);
    let sw = gtk::ScrolledWindow::builder().child(&tv).vexpand(true).build();
    let save = gtk::Button::with_label(&gettext("Save"));
    save.add_css_class("suggested-action");
    let header = adw::HeaderBar::new();
    header.pack_end(&save);
    let tbv = adw::ToolbarView::new();
    tbv.add_top_bar(&header);
    tbv.set_content(Some(&sw));
    let page = adw::NavigationPage::builder().title(title).child(&tbv).build();
    let d = dialog.clone();
    save.connect_clicked(move |_| {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&path, text_of(&tv)) {
            Ok(()) => {
                d.add_toast(adw::Toast::new(&gettext("Saved")));
                d.pop_subpage();
            }
            Err(e) => d.add_toast(adw::Toast::new(&gettext_f("Could not save: {error}", &[("error", &e.to_string())]))),
        }
    });
    dialog.push_subpage(&page);
}

/// Add or edit a remote.
pub async fn edit_remote(rv: &Rc<RepoView>, name: Option<String>) {
    let existing = name
        .as_ref()
        .and_then(|n| rv.snapshot().remotes.iter().find(|r| &r.name == n).cloned());
    let form = Form::new(&if existing.is_some() { gettext("Edit Remote") } else { gettext("Add Remote") }, &gettext("OK"));
    let n = form.entry(&gettext("Remote name"), existing.as_ref().map(|r| r.name.as_str()).unwrap_or(if rv.snapshot().remotes.is_empty() { "origin" } else { "" }));
    let url = form.entry(&gettext("URL / path"), existing.as_ref().map(|r| r.fetch_url.as_str()).unwrap_or(""));
    let push_url = form.entry(
        &gettext("Push URL (optional)"),
        existing
            .as_ref()
            .filter(|r| r.push_url != r.fetch_url)
            .map(|r| r.push_url.as_str())
            .unwrap_or(""),
    );
    let fetch_now = form.switch(&gettext("Fetch after saving"), "", existing.is_none());
    let (n2, u2) = (n.clone(), url.clone());
    form.watch(&n);
    form.watch(&url);
    form.validate(move || !n2.text().trim().is_empty() && !u2.text().trim().is_empty());
    form.focus(&url);
    if !form.run(&rv.widget).await {
        return;
    }
    let new_name = n.text().trim().to_string();
    let u = url.text().trim().to_string();
    let pu = push_url.text().trim().to_string();
    let mut cmds = Vec::new();
    match &existing {
        Some(r) => {
            if r.name != new_name {
                cmds.push(s(&["remote", "rename", &r.name, &new_name]));
            }
            if r.fetch_url != u {
                cmds.push(s(&["remote", "set-url", &new_name, &u]));
            }
            if !pu.is_empty() && pu != r.push_url {
                cmds.push(s(&["remote", "set-url", "--push", &new_name, &pu]));
            }
        }
        None => {
            cmds.push(s(&["remote", "add", &new_name, &u]));
            if !pu.is_empty() {
                cmds.push(s(&["remote", "set-url", "--push", &new_name, &pu]));
            }
        }
    }
    if fetch_now.is_active() {
        cmds.push(s(&["fetch", "--progress", &new_name]));
    }
    if cmds.is_empty() {
        return;
    }
    rv.run_ops(
        &gettext("Remote"),
        cmds,
        OpOptions {
            network: fetch_now.is_active(),
            ..Default::default()
        },
    )
    .await;
}

/// Git LFS: install hooks, manage tracked patterns, fetch/pull/push/prune.
pub async fn lfs_dialog(rv: &Rc<RepoView>) {
    let git = rv.git.clone();
    let (installed, patterns) = bg(move || {
        let installed = git.check(&["lfs", "version"]);
        let patterns: Vec<String> = git
            .run(&["lfs", "track"])
            .map(|out| {
                out.lines()
                    .filter(|l| l.starts_with("    "))
                    .filter_map(|l| l.trim().split(" (").next().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        (installed, patterns)
    })
    .await;
    if !installed {
        show_error(&rv.widget, &gettext("Git LFS is not installed"), &gettext("Install the git-lfs package (e.g. `sudo apt install git-lfs`)."));
        return;
    }
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&gettext("Git LFS"));
    dialog.set_search_enabled(false);
    let page = adw::PreferencesPage::new();

    let actions = adw::PreferencesGroup::builder().title(gettext("Actions")).build();
    let action_row = |title: &str, sub: &str, cmd: Vec<String>, network: bool| {
        let row = adw::ActionRow::builder().title(title).subtitle(sub).activatable(true).build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (rv2, d2, t) = (rv.clone(), dialog.clone(), title.to_string());
        row.connect_activated(move |_| {
            d2.close();
            let rv3 = rv2.clone();
            let cmd = cmd.clone();
            let t = t.clone();
            spawn(async move {
                rv3.run_ops(&t, vec![cmd], OpOptions { network, ..Default::default() }).await;
            });
        });
        row
    };
    actions.add(&action_row(&gettext("Initialise LFS in this repository"), "git lfs install --local", s(&["lfs", "install", "--local"]), false));
    actions.add(&action_row(&gettext("Fetch"), &gettext("Download LFS objects for the current branch"), s(&["lfs", "fetch"]), true));
    actions.add(&action_row(&gettext("Pull"), &gettext("Fetch and check out LFS files"), s(&["lfs", "pull"]), true));
    actions.add(&action_row(&gettext("Push"), &gettext("Upload LFS objects for all branches"), s(&["lfs", "push", "--all", "origin"]), true));
    actions.add(&action_row(&gettext("Prune"), &gettext("Delete old local LFS files"), s(&["lfs", "prune"]), false));
    page.add(&actions);

    let tracked = adw::PreferencesGroup::builder()
        .title(gettext("Tracked patterns"))
        .description(gettext("Stored in .gitattributes"))
        .build();
    let add_row = adw::EntryRow::builder().title(gettext("Add pattern (e.g. *.psd)")).show_apply_button(true).build();
    let (rv2, d2) = (rv.clone(), dialog.clone());
    add_row.connect_apply(move |r| {
        let p = r.text().trim().to_string();
        if p.is_empty() {
            return;
        }
        d2.close();
        let rv3 = rv2.clone();
        spawn(async move {
            rv3.run_ops(&gettext("LFS Track"), vec![s(&["lfs", "track", &p]), s(&["add", ".gitattributes"])], OpOptions::default()).await;
        });
    });
    tracked.add(&add_row);
    for p in patterns {
        let row = adw::ActionRow::builder().title(glib_escape(&p)).build();
        let del = gtk::Button::from_icon_name("user-trash-symbolic");
        del.add_css_class("flat");
        del.set_valign(gtk::Align::Center);
        del.set_tooltip_text(Some(&gettext("Untrack")));
        row.add_suffix(&del);
        let (rv2, d2) = (rv.clone(), dialog.clone());
        del.connect_clicked(move |_| {
            d2.close();
            let rv3 = rv2.clone();
            let p = p.clone();
            spawn(async move {
                rv3.run_ops(&gettext("LFS Untrack"), vec![s(&["lfs", "untrack", &p]), s(&["add", ".gitattributes"])], OpOptions::default()).await;
            });
        });
        tracked.add(&row);
    }
    page.add(&tracked);
    dialog.add(&page);
    dialog.present(Some(&rv.widget));
}

fn glib_escape(s: &str) -> String {
    gtk::glib::markup_escape_text(s).to_string()
}

#[allow(dead_code)]
fn _check(_: git::GitError) {}
