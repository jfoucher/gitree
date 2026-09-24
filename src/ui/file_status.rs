//! File Status view: staged / unstaged file lists, diff with hunk and line
//! staging, and the commit box.

use super::diff_view::DiffView;
use super::progress::OpOptions;
use super::repo_view::{RepoView, Snapshot};
use super::staging::StagingView;
use super::{bg, spawn};
use crate::config;
use crate::git::state::OpState;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    Pending,
    Conflicted,
    Untracked,
    Modified,
    Ignored,
}

const FILTERS: [(&str, Filter); 5] = [
    ("Pending files", Filter::Pending),
    ("Conflicted files", Filter::Conflicted),
    ("Untracked files", Filter::Untracked),
    ("Modified files", Filter::Modified),
    ("Ignored files", Filter::Ignored),
];

pub struct FileStatusView {
    pub widget: gtk::Paned,
    rv: Weak<RepoView>,
    staging: Rc<StagingView>,
    pub diff: Rc<DiffView>,
    filter: gtk::DropDown,
    search: gtk::SearchEntry,
    // Commit box
    message: gtk::TextView,
    amend: gtk::CheckButton,
    signoff: gtk::CheckButton,
    no_verify: gtk::CheckButton,
    gpg: gtk::CheckButton,
    push_after: gtk::CheckButton,
    author: gtk::Label,
    commit_btn: gtk::Button,
    history_btn: gtk::MenuButton,
    last_op: RefCell<OpState>,
}

impl FileStatusView {
    pub fn new(rv: Weak<RepoView>) -> Rc<Self> {
        let staging = StagingView::new(rv.clone(), 220, false);
        let lists = staging.lists.clone();

        // Filter bar
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("pane-header");
        let names: Vec<&str> = FILTERS.iter().map(|(n, _)| *n).collect();
        let filter = gtk::DropDown::from_strings(&names);
        filter.set_tooltip_text(Some("Show files"));
        bar.append(&filter);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Filter files")
            .hexpand(true)
            .build();
        bar.append(&search);
        let tree_btn = gtk::ToggleButton::builder()
            .icon_name("view-list-bullet-symbolic")
            .tooltip_text("Tree view")
            .active(config::with(|s| s.file_tree_view))
            .build();
        tree_btn.add_css_class("flat");
        bar.append(&tree_btn);

        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.append(&bar);
        left.append(&lists);

        let diff = staging.diff.clone();
        let top = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&left)
            .end_child(&diff.widget)
            .position(380)
            .shrink_start_child(false)
            .build();

        // Commit box
        let cbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        cbox.add_css_class("commit-box");
        let crow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let avatar = gtk::Image::from_icon_name("avatar-default-symbolic");
        let author = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        author.add_css_class("dim-label");
        crow.append(&avatar);
        crow.append(&author);

        let amend = gtk::CheckButton::with_label("Amend last commit");
        let signoff = gtk::CheckButton::with_label("Sign off");
        let no_verify = gtk::CheckButton::with_label("Bypass commit hooks");
        let gpg = gtk::CheckButton::with_label("Sign commit (GPG)");
        let opts_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        opts_box.set_margin_top(6);
        opts_box.set_margin_bottom(6);
        opts_box.set_margin_start(6);
        opts_box.set_margin_end(6);
        opts_box.append(&amend);
        opts_box.append(&signoff);
        opts_box.append(&no_verify);
        opts_box.append(&gpg);
        let opts_pop = gtk::Popover::builder().child(&opts_box).build();
        let opts_btn = gtk::MenuButton::builder()
            .label("Commit Options")
            .popover(&opts_pop)
            .build();
        opts_btn.add_css_class("flat");
        crow.append(&opts_btn);
        let history_btn = gtk::MenuButton::builder()
            .icon_name("document-open-recent-symbolic")
            .tooltip_text("Previous commit messages")
            .build();
        history_btn.add_css_class("flat");
        crow.append(&history_btn);
        cbox.append(&crow);

        let message = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .accepts_tab(false)
            .build();
        let msg_sw = gtk::ScrolledWindow::builder()
            .child(&message)
            .min_content_height(70)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        msg_sw.add_css_class("card");
        cbox.append(&msg_sw);

        let brow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let push_after = gtk::CheckButton::with_label("Push changes immediately");
        push_after.set_active(config::with(|s| s.push_after_commit));
        push_after.set_hexpand(true);
        brow.append(&push_after);
        let hint = gtk::Label::new(Some("Ctrl+Enter to commit"));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        brow.append(&hint);
        let commit_btn = gtk::Button::with_label("Commit");
        commit_btn.add_css_class("suggested-action");
        brow.append(&commit_btn);
        cbox.append(&brow);

        let widget = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&top)
            .end_child(&cbox)
            .resize_end_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        // Place the commit box near the bottom once we know our height.
        widget.connect_realize(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || {
                let h = p.height();
                if h > 300 {
                    p.set_position(h - 170);
                }
            });
        });

        let this = Rc::new(Self {
            widget,
            rv,
            staging,
            diff,
            filter,
            search,
            message,
            amend,
            signoff,
            no_verify,
            gpg,
            push_after,
            author,
            commit_btn,
            history_btn,
            last_op: RefCell::new(OpState::None),
        });

        let w = Rc::downgrade(&this);
        tree_btn.connect_toggled(move |b| {
            config::update(|s| s.file_tree_view = b.is_active());
            if let Some(t) = w.upgrade() {
                t.staging.set_tree_mode(b.is_active());
            }
        });
        this.connect_commit_box();

        let w = Rc::downgrade(&this);
        this.filter.connect_selected_notify(move |dd| {
            if let Some(t) = w.upgrade() {
                let ign = FILTERS[dd.selected() as usize].1 == Filter::Ignored;
                if let Some(rv) = t.rv.upgrade() {
                    if rv.include_ignored.replace(ign) != ign {
                        rv.refresh();
                    } else {
                        t.update(&rv.snapshot());
                    }
                }
            }
        });
        let w = Rc::downgrade(&this);
        this.search.connect_search_changed(move |_| {
            if let Some(t) = w.upgrade()
                && let Some(rv) = t.rv.upgrade() {
                    t.update(&rv.snapshot());
                }
        });
        this
    }

    fn connect_commit_box(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.commit_btn.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                t.commit();
            }
        });
        let key = gtk::EventControllerKey::new();
        let w = Rc::downgrade(self);
        key.connect_key_pressed(move |_, k, _, m| {
            if (k == gtk::gdk::Key::Return || k == gtk::gdk::Key::KP_Enter)
                && m.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                if let Some(t) = w.upgrade() {
                    t.commit();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.message.add_controller(key);

        let w = Rc::downgrade(self);
        self.amend.connect_toggled(move |b| {
            let Some(t) = w.upgrade() else { return };
            let Some(rv) = t.rv.upgrade() else { return };
            if b.is_active() {
                let buf = t.message.buffer();
                if super::form::text_of(&t.message).trim().is_empty()
                    && let Ok(m) = rv.git.run(&["log", "-1", "--format=%B"]) {
                        buf.set_text(m.trim_end());
                    }
                t.commit_btn.set_label("Amend Commit");
            } else {
                t.commit_btn.set_label("Commit");
            }
        });
        let w = Rc::downgrade(self);
        self.push_after.connect_toggled(move |b| {
            config::update(|s| s.push_after_commit = b.is_active());
            let _ = w.upgrade();
        });

        // Commit message history popover, rebuilt when opened.
        let pop = gtk::Popover::new();
        self.history_btn.set_popover(Some(&pop));
        let w = Rc::downgrade(self);
        pop.connect_show(move |pop| {
            let Some(t) = w.upgrade() else { return };
            let Some(rv) = t.rv.upgrade() else { return };
            let key = rv.git.workdir.to_string_lossy().to_string();
            let msgs = config::with(|s| s.commit_history.get(&key).cloned().unwrap_or_default());
            let list = gtk::ListBox::new();
            list.add_css_class("navigation-sidebar");
            if msgs.is_empty() {
                let l = gtk::Label::new(Some("No previous messages"));
                l.set_margin_top(12);
                l.set_margin_bottom(12);
                l.add_css_class("dim-label");
                list.append(&l);
            }
            for m in &msgs {
                let first = m.lines().next().unwrap_or("");
                let l = gtk::Label::builder()
                    .label(first)
                    .xalign(0.0)
                    .max_width_chars(60)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build();
                list.append(&l);
            }
            let t2 = Rc::downgrade(&t);
            let p2 = pop.clone();
            list.connect_row_activated(move |_, row| {
                if let (Some(t), Some(m)) = (t2.upgrade(), msgs.get(row.index() as usize)) {
                    t.message.buffer().set_text(m);
                }
                p2.popdown();
            });
            let sw = gtk::ScrolledWindow::builder()
                .child(&list)
                .propagate_natural_height(true)
                .max_content_height(360)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            pop.set_child(Some(&sw));
        });
    }

    pub fn debug_select(&self, staged: bool, index: u32) {
        self.staging.debug_select(staged, index);
    }

    pub fn stage_paths(&self, paths: Vec<String>) {
        self.staging.stage_paths(paths);
    }

    pub fn unstage_paths(&self, paths: Vec<String>) {
        self.staging.unstage_paths(paths);
    }


    /// Focuses the commit message (toolbar "Commit" button).
    pub fn focus_commit(&self) {
        self.message.grab_focus();
    }

    pub fn update(self: &Rc<Self>, s: &Snapshot) {
        let filter = FILTERS[self.filter.selected() as usize].1;
        let q = self.search.text().to_lowercase();
        self.staging.update(s, |e, code| {
            (q.is_empty() || e.path.to_lowercase().contains(&q))
                && match filter {
                    Filter::Pending => !e.ignored,
                    Filter::Conflicted => e.conflicted,
                    Filter::Untracked => e.untracked,
                    Filter::Modified => code == 'M',
                    Filter::Ignored => e.ignored,
                }
        });
        // Commit box state.
        if let Some(rv) = self.rv.upgrade() {
            let git = rv.git.clone();
            let author = self.author.clone();
            spawn(async move {
                let (n, e) = bg(move || {
                    (
                        crate::git::config_get(Some(&git), "user.name").unwrap_or_else(|| "Unknown user".into()),
                        crate::git::config_get(Some(&git), "user.email").unwrap_or_default(),
                    )
                })
                .await;
                author.set_text(&format!("{n} <{e}>"));
            });
            let target = s
                .refs
                .current()
                .and_then(|r| r.upstream.clone())
                .or_else(|| {
                    s.current_branch()
                        .zip(s.default_remote())
                        .map(|(b, r)| format!("{r}/{b}"))
                });
            match target {
                Some(t) => {
                    self.push_after.set_label(Some(&format!("Push changes immediately to {t}")));
                    self.push_after.set_sensitive(true);
                }
                None => {
                    self.push_after.set_label(Some("Push changes immediately (no remote)"));
                    self.push_after.set_sensitive(false);
                }
            }
            // Pre-fill the merge message when a merge starts.
            let prev = self.last_op.replace(s.op.clone());
            if prev != s.op && matches!(s.op, OpState::Merge | OpState::CherryPick | OpState::Revert)
                && super::form::text_of(&self.message).trim().is_empty()
                    && let Some(m) = crate::git::state::merge_message(&rv.git_dir) {
                        self.message.buffer().set_text(&m);
                    }
        }
    }


    pub fn commit(self: &Rc<Self>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let msg = super::form::text_of(&self.message);
        let snap = rv.snapshot();
        if msg.trim().is_empty() {
            rv.toast("Please enter a commit message");
            self.message.grab_focus();
            return;
        }
        let amend = self.amend.is_active();
        let has_staged = snap.status.staged().next().is_some();
        if !amend && !has_staged && !matches!(snap.op, OpState::Merge) {
            super::show_error(
                &self.widget,
                "Nothing to commit",
                "Stage the changes you want to commit first (tick them in the Unstaged files list).",
            );
            return;
        }
        if snap.status.has_conflicts() {
            super::show_error(
                &self.widget,
                "Unresolved conflicts",
                "Resolve all conflicted files and mark them resolved before committing.",
            );
            return;
        }
        let msg_file = rv.git_dir.join("GITREE_COMMIT_MSG");
        if let Err(e) = std::fs::write(&msg_file, &msg) {
            super::show_error(&self.widget, "Could not write commit message", &e.to_string());
            return;
        }
        let mut cmd = vec![
            "commit".to_string(),
            "--cleanup=strip".to_string(),
            "-F".to_string(),
            msg_file.to_string_lossy().to_string(),
        ];
        if amend {
            cmd.push("--amend".into());
        }
        if self.signoff.is_active() {
            cmd.push("--signoff".into());
        }
        if self.no_verify.is_active() {
            cmd.push("--no-verify".into());
        }
        if self.gpg.is_active() {
            cmd.push("-S".into());
        }
        let mut cmds = vec![cmd];
        let push = self.push_after.is_active() && self.push_after.is_sensitive();
        if push
            && let (Some(branch), Some(remote)) = (snap.current_branch(), snap.default_remote()) {
                let has_upstream = snap.refs.current().is_some_and(|r| r.upstream.is_some());
                let mut p = vec!["push".to_string(), "--progress".to_string()];
                if amend {
                    p.push("--force-with-lease".into());
                }
                if !has_upstream {
                    p.push("-u".into());
                }
                p.push(remote);
                p.push(branch.to_string());
                cmds.push(p);
            }
        let this = self.clone();
        let key = rv.git.workdir.to_string_lossy().to_string();
        spawn(async move {
            let ok = rv
                .run_ops(
                    if push { "Commit and Push" } else { "Commit" },
                    cmds,
                    OpOptions {
                        network: push,
                        ..Default::default()
                    },
                )
                .await;
            let _ = std::fs::remove_file(&msg_file);
            if ok {
                config::update(|s| s.remember_message(&key, msg.trim()));
                this.message.buffer().set_text("");
                this.amend.set_active(false);
                rv.toast("Committed");
            }
        });
    }
}
