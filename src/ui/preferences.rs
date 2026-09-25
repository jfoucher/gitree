//! Application preferences.

use crate::config::{self, CustomAction};
use crate::git;
use crate::i18n::{gettext, gettext_f};
use adw::prelude::*;
use std::path::PathBuf;

pub fn show(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&gettext("Preferences"));

    // General
    let general = adw::PreferencesPage::builder()
        .title(gettext("General"))
        .icon_name("preferences-system-symbolic")
        .build();
    let ident = adw::PreferencesGroup::builder()
        .title(gettext("Default user information"))
        .description(gettext("Global git identity used for commits (git config --global)"))
        .build();
    let get = |k: &str| git::config_get(None, k).unwrap_or_default();
    for (title, key) in [(gettext("Full name"), "user.name"), (gettext("Email address"), "user.email")] {
        let row = adw::EntryRow::builder()
            .title(title)
            .text(get(key))
            .show_apply_button(true)
            .build();
        let key = key.to_string();
        row.connect_apply(move |r| {
            let _ = git::run_global(&["config", "--global", &key, r.text().trim()]);
        });
        ident.add(&row);
    }
    general.add(&ident);

    let repos = adw::PreferencesGroup::builder().title(gettext("Repositories")).build();
    let clone_dir = adw::EntryRow::builder()
        .title(gettext("Default clone folder"))
        .text(
            config::with(|s| s.default_clone_dir.clone())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
        .show_apply_button(true)
        .build();
    clone_dir.connect_apply(|r| {
        let p = PathBuf::from(super::browser::expand_tilde(r.text().trim()));
        config::update(|s| s.default_clone_dir = Some(p));
    });
    repos.add(&clone_dir);
    let fetch = adw::SpinRow::with_range(0.0, 240.0, 1.0);
    fetch.set_title(&gettext("Check remotes every (minutes)"));
    fetch.set_subtitle(&gettext("Background fetch; 0 disables it"));
    fetch.set_value(config::with(|s| s.fetch_interval_min) as f64);
    fetch.connect_value_notify(|r| {
        let v = r.value() as u32;
        config::update(|s| s.fetch_interval_min = v);
    });
    repos.add(&fetch);
    let page_size = adw::SpinRow::with_range(200.0, 100000.0, 100.0);
    page_size.set_title(&gettext("Commits loaded at a time"));
    page_size.set_value(config::with(|s| s.log_page_size) as f64);
    page_size.connect_value_notify(|r| {
        let v = r.value() as u32;
        config::update(|s| s.log_page_size = v);
    });
    repos.add(&page_size);
    let confirm = adw::SwitchRow::builder()
        .title(gettext("Confirm dangerous operations"))
        .subtitle(gettext("Hard reset and similar"))
        .active(config::with(|s| s.confirm_dangerous))
        .build();
    confirm.connect_active_notify(|r| {
        let v = r.is_active();
        config::update(|s| s.confirm_dangerous = v);
    });
    repos.add(&confirm);
    let term = adw::EntryRow::builder()
        .title(gettext("Terminal command (empty = auto-detect)"))
        .text(config::with(|s| s.terminal.clone()))
        .show_apply_button(true)
        .build();
    term.connect_apply(|r| {
        let v = r.text().trim().to_string();
        config::update(|s| s.terminal = v);
    });
    repos.add(&term);
    general.add(&repos);

    let creds = adw::PreferencesGroup::builder()
        .title(gettext("Credentials"))
        .description(gettext("How git remembers HTTPS passwords (git config --global credential.helper)"))
        .build();
    let helpers = ["(none)", "cache --timeout=3600", "store", "libsecret", "manager"];
    let current = git::config_get(None, "credential.helper").unwrap_or_default();
    let none_label = gettext("(none)");
    let model = gtk::StringList::new(&[&none_label, helpers[1], helpers[2], helpers[3], helpers[4]]);
    let combo = adw::ComboRow::builder().title(gettext("Credential helper")).model(&model).build();
    let idx = helpers.iter().position(|h| *h == current || (current.is_empty() && *h == "(none)"));
    if let Some(i) = idx {
        combo.set_selected(i as u32);
    } else {
        combo.set_subtitle(&gettext_f("Currently: {helper}", &[("helper", &current)]));
        combo.set_selected(gtk::INVALID_LIST_POSITION);
    }
    combo.connect_selected_notify(move |c| {
        let i = c.selected() as usize;
        if let Some(h) = helpers.get(i) {
            let _ = if *h == "(none)" {
                git::run_global(&["config", "--global", "--unset-all", "credential.helper"])
            } else {
                git::run_global(&["config", "--global", "credential.helper", h])
            };
        }
    });
    creds.add(&combo);
    let ssh = adw::ActionRow::builder()
        .title(gettext("SSH keys"))
        .subtitle(gettext("SSH authentication uses your ~/.ssh keys and ssh-agent. Passphrases are asked with a dialog."))
        .build();
    creds.add(&ssh);
    general.add(&creds);
    dialog.add(&general);

    // Diff & tools
    let diff = adw::PreferencesPage::builder()
        .title(gettext("Diff"))
        .icon_name("gitree-filestatus-symbolic")
        .build();
    let dg = adw::PreferencesGroup::builder().title(gettext("Diff view")).build();
    let ctx = adw::SpinRow::with_range(0.0, 100.0, 1.0);
    ctx.set_title(&gettext("Lines of context"));
    ctx.set_value(config::with(|s| s.diff_context) as f64);
    ctx.connect_value_notify(|r| {
        let v = r.value() as u32;
        config::update(|s| s.diff_context = v);
    });
    dg.add(&ctx);
    let ws = adw::SwitchRow::builder()
        .title(gettext("Ignore whitespace by default"))
        .active(config::with(|s| s.diff_ignore_whitespace))
        .build();
    ws.connect_active_notify(|r| {
        let v = r.is_active();
        config::update(|s| s.diff_ignore_whitespace = v);
    });
    dg.add(&ws);
    diff.add(&dg);
    let tools = adw::PreferencesGroup::builder()
        .title(gettext("External tools"))
        .description(gettext("A git tool name (meld, kdiff3, vimdiff, bc, …) or a command using $LOCAL $REMOTE ($BASE $MERGED for merges)"))
        .build();
    let dt = adw::EntryRow::builder()
        .title(gettext("Diff tool"))
        .text(config::with(|s| s.diff_tool.clone()))
        .show_apply_button(true)
        .build();
    dt.connect_apply(|r| {
        let v = r.text().trim().to_string();
        config::update(|s| s.diff_tool = v);
    });
    tools.add(&dt);
    let mt = adw::EntryRow::builder()
        .title(gettext("Merge tool"))
        .text(config::with(|s| s.merge_tool.clone()))
        .show_apply_button(true)
        .build();
    mt.connect_apply(|r| {
        let v = r.text().trim().to_string();
        config::update(|s| s.merge_tool = v);
    });
    tools.add(&mt);
    diff.add(&tools);
    dialog.add(&diff);

    // Custom actions
    let custom = adw::PreferencesPage::builder()
        .title(gettext("Custom Actions"))
        .icon_name("system-run-symbolic")
        .build();
    let cg = adw::PreferencesGroup::builder()
        .title(gettext("Custom actions"))
        .description(gettext("Shown in the commit and file context menus. Arguments may use $REPO, $SHA and $FILE."))
        .build();
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.add_css_class("flat");
    cg.set_header_suffix(Some(&add));
    fill_custom(&cg);
    let cg2 = cg.clone();
    let d2 = dialog.clone();
    add.connect_clicked(move |_| {
        let cg3 = cg2.clone();
        let d3 = d2.clone();
        glib_spawn(async move {
            let form = super::form::Form::new(&gettext("New Custom Action"), &gettext("Add"));
            let name = form.entry(&gettext("Menu caption"), "");
            let cmd = form.entry(&gettext("Script to run"), "");
            let args = form.entry(&gettext("Parameters"), "$SHA");
            let show = form.switch(&gettext("Show full output"), "", false);
            let (n2, c2) = (name.clone(), cmd.clone());
            form.watch(&name);
            form.watch(&cmd);
            form.validate(move || !n2.text().trim().is_empty() && !c2.text().trim().is_empty());
            form.focus(&name);
            if form.run(&d3).await {
                config::update(|s| {
                    s.custom_actions.push(CustomAction {
                        name: name.text().trim().to_string(),
                        command: cmd.text().trim().to_string(),
                        args: args.text().trim().to_string(),
                        show_output: show.is_active(),
                    })
                });
                fill_custom(&cg3);
            }
        });
    });
    custom.add(&cg);
    dialog.add(&custom);

    dialog.present(Some(parent));
}

fn glib_spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    gtk::glib::spawn_future_local(f);
}

thread_local! {
    static CUSTOM_ROWS: std::cell::RefCell<Vec<adw::ActionRow>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn fill_custom(group: &adw::PreferencesGroup) {
    CUSTOM_ROWS.with(|rows| {
        for r in rows.borrow_mut().drain(..) {
            group.remove(&r);
        }
        for (i, a) in config::with(|s| s.custom_actions.clone()).into_iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(&a.name))
                .subtitle(gtk::glib::markup_escape_text(&format!("{} {}", a.command, a.args)))
                .build();
            let del = gtk::Button::from_icon_name("user-trash-symbolic");
            del.add_css_class("flat");
            del.set_valign(gtk::Align::Center);
            let g = group.clone();
            del.connect_clicked(move |_| {
                config::update(|s| {
                    if i < s.custom_actions.len() {
                        s.custom_actions.remove(i);
                    }
                });
                fill_custom(&g);
            });
            row.add_suffix(&del);
            group.add(&row);
            rows.borrow_mut().push(row);
        }
    });
}
