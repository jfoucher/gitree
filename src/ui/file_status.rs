//! File Status view: staged / unstaged file lists, diff with hunk and line
//! staging, and the commit box.

use super::diff_view::{DiffContext, DiffKind, DiffView, PatchAction};
use super::file_list::{FileItem, FileList};
use super::progress::OpOptions;
use super::repo_view::{RepoView, Snapshot};
use super::{bg, menu_item_target, popup_menu, spawn};
use crate::config;
use crate::git::diff::{self, FileDiff, Hunk, LineKind};
use crate::git::state::OpState;
use crate::git::Git;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
}

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

/// Encodes a side and paths as an action target: "S|a\nb".
pub fn encode_target(side: Side, paths: &[String]) -> String {
    format!(
        "{}|{}",
        if side == Side::Staged { "S" } else { "U" },
        paths.join("\n")
    )
}

pub fn decode_target(t: &str) -> (Side, Vec<String>) {
    let (s, rest) = t.split_once('|').unwrap_or(("U", t));
    let side = if s == "S" { Side::Staged } else { Side::Unstaged };
    (side, rest.split('\n').filter(|p| !p.is_empty()).map(String::from).collect())
}

pub struct FileStatusView {
    pub widget: gtk::Paned,
    rv: Weak<RepoView>,
    staged: Rc<FileList>,
    unstaged: Rc<FileList>,
    staged_count: gtk::Label,
    unstaged_count: gtk::Label,
    pub diff: Rc<DiffView>,
    filter: gtk::DropDown,
    search: gtk::SearchEntry,
    selected: RefCell<Option<(Side, Vec<FileItem>)>>,
    diff_gen: Cell<u64>,
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

fn pane(title: &str, list: &Rc<FileList>, buttons: &[&gtk::Button]) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("pane-header");
    let l = gtk::Label::builder().label(title).xalign(0.0).build();
    l.add_css_class("title");
    let count = gtk::Label::new(None);
    count.add_css_class("dim-label");
    count.add_css_class("caption");
    count.set_hexpand(true);
    count.set_xalign(0.0);
    header.append(&l);
    header.append(&count);
    for btn in buttons {
        btn.add_css_class("flat");
        btn.add_css_class("caption");
        header.append(*btn);
    }
    b.append(&header);
    b.append(&list.widget);
    (b, count)
}

impl FileStatusView {
    pub fn new(rv: Weak<RepoView>) -> Rc<Self> {
        let staged = FileList::new(Some(true));
        let unstaged = FileList::new(Some(false));
        let unstage_all = gtk::Button::with_label("Unstage All");
        let unstage_sel = gtk::Button::with_label("Unstage Selected");
        let stage_all = gtk::Button::with_label("Stage All");
        let stage_sel = gtk::Button::with_label("Stage Selected");
        let (staged_pane, staged_count) = pane("Staged files", &staged, &[&unstage_all, &unstage_sel]);
        let (unstaged_pane, unstaged_count) = pane("Unstaged files", &unstaged, &[&stage_all, &stage_sel]);

        let lists = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&staged_pane)
            .end_child(&unstaged_pane)
            .position(220)
            .build();

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
        lists.set_vexpand(true);

        let diff = DiffView::new();
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
            staged,
            unstaged,
            staged_count,
            unstaged_count,
            diff,
            filter,
            search,
            selected: RefCell::new(None),
            diff_gen: Cell::new(0),
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
        this.staged.set_tree_mode(tree_btn.is_active());
        this.unstaged.set_tree_mode(tree_btn.is_active());

        let w = Rc::downgrade(&this);
        tree_btn.connect_toggled(move |b| {
            config::update(|s| s.file_tree_view = b.is_active());
            if let Some(t) = w.upgrade() {
                t.staged.set_tree_mode(b.is_active());
                t.unstaged.set_tree_mode(b.is_active());
            }
        });
        this.connect_lists();
        this.connect_buttons(&stage_all, &stage_sel, &unstage_all, &unstage_sel);
        this.connect_commit_box();
        this.connect_diff();

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

    fn connect_lists(self: &Rc<Self>) {
        for (list, side) in [(&self.staged, Side::Staged), (&self.unstaged, Side::Unstaged)] {
            let w = Rc::downgrade(self);
            *list.on_selection.borrow_mut() = Some(Box::new(move |files| {
                let Some(t) = w.upgrade() else { return };
                if files.is_empty() {
                    let still = t.selected.borrow().as_ref().is_some_and(|(s, _)| *s == side);
                    if still {
                        *t.selected.borrow_mut() = None;
                        t.diff.show_message("No file selected");
                    }
                    return;
                }
                // Selecting in one list clears the other (like Sourcetree).
                match side {
                    Side::Staged => t.unstaged.unselect_all(),
                    Side::Unstaged => t.staged.unselect_all(),
                }
                *t.selected.borrow_mut() = Some((side, files));
                t.load_diff();
            }));
            let w = Rc::downgrade(self);
            *list.on_toggle.borrow_mut() = Some(Box::new(move |files| {
                let Some(t) = w.upgrade() else { return };
                let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
                match side {
                    Side::Unstaged => t.stage_paths(paths),
                    Side::Staged => t.unstage_paths(paths),
                }
            }));
            let w = Rc::downgrade(self);
            *list.on_activate.borrow_mut() = Some(Box::new(move |files| {
                let Some(t) = w.upgrade() else { return };
                let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
                match side {
                    Side::Unstaged => t.stage_paths(paths),
                    Side::Staged => t.unstage_paths(paths),
                }
            }));
            let w = Rc::downgrade(self);
            *list.on_context.borrow_mut() = Some(Box::new(move |widget, x, y, files| {
                if let Some(t) = w.upgrade() {
                    t.context_menu(widget, x, y, side, &files);
                }
            }));
        }
    }

    fn connect_buttons(self: &Rc<Self>, stage_all: &gtk::Button, stage_sel: &gtk::Button, unstage_all: &gtk::Button, unstage_sel: &gtk::Button) {
        let w = Rc::downgrade(self);
        stage_all.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                let paths: Vec<String> = t.unstaged.all_files().into_iter().map(|f| f.path).collect();
                if !paths.is_empty() {
                    t.stage_paths(paths);
                }
            }
        });
        let w = Rc::downgrade(self);
        stage_sel.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                let paths: Vec<String> = t.unstaged.selected_files().into_iter().map(|f| f.path).collect();
                if !paths.is_empty() {
                    t.stage_paths(paths);
                }
            }
        });
        let w = Rc::downgrade(self);
        unstage_all.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                let paths: Vec<String> = t.staged.all_files().into_iter().map(|f| f.path).collect();
                if !paths.is_empty() {
                    t.unstage_paths(paths);
                }
            }
        });
        let w = Rc::downgrade(self);
        unstage_sel.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                let paths: Vec<String> = t.staged.selected_files().into_iter().map(|f| f.path).collect();
                if !paths.is_empty() {
                    t.unstage_paths(paths);
                }
            }
        });
    }

    fn connect_diff(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        *self.diff.on_options_changed.borrow_mut() = Some(Box::new(move |()| {
            if let Some(t) = w.upgrade() {
                t.load_diff();
            }
        }));
        let w = Rc::downgrade(self);
        *self.diff.on_patch.borrow_mut() = Some(Box::new(move |action, file, sel| {
            if let Some(t) = w.upgrade() {
                t.apply_patch_action(action, file, sel);
            }
        }));
        let w = Rc::downgrade(self);
        *self.diff.on_external.borrow_mut() = Some(Box::new(move |path| {
            if let Some(t) = w.upgrade() {
                let staged = t.selected.borrow().as_ref().is_some_and(|(s, _)| *s == Side::Staged);
                if let Some(rv) = t.rv.upgrade() {
                    super::dialogs::external_diff(&rv, &path, staged, None);
                }
            }
        }));
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
        let list = if staged { &self.staged } else { &self.unstaged };
        list.selection.select_item(index, true);
    }

    /// Focuses the commit message (toolbar "Commit" button).
    pub fn focus_commit(&self) {
        self.message.grab_focus();
    }

    pub fn update(self: &Rc<Self>, s: &Snapshot) {
        let filter = FILTERS[self.filter.selected() as usize].1;
        let q = self.search.text().to_lowercase();
        let keep = |path: &str| q.is_empty() || path.to_lowercase().contains(&q);
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        for e in &s.status.entries {
            if !keep(&e.path) {
                continue;
            }
            let item_for = |code: char| FileItem {
                path: e.path.clone(),
                orig_path: e.orig_path.clone(),
                code,
                untracked: e.untracked,
                conflicted: e.conflicted,
            };
            let pass = |code: char| match filter {
                Filter::Pending => !e.ignored,
                Filter::Conflicted => e.conflicted,
                Filter::Untracked => e.untracked,
                Filter::Modified => code == 'M',
                Filter::Ignored => e.ignored,
            };
            if e.is_staged() && pass(e.staged_code()) {
                staged.push(item_for(e.staged_code()));
            }
            if e.is_unstaged() && pass(e.unstaged_code()) {
                unstaged.push(item_for(e.unstaged_code()));
            }
        }
        self.staged_count.set_text(&if staged.is_empty() { String::new() } else { format!("{}", staged.len()) });
        self.unstaged_count.set_text(&if unstaged.is_empty() { String::new() } else { format!("{}", unstaged.len()) });
        self.staged.set_files(staged);
        self.unstaged.set_files(unstaged);

        // Refresh the diff: working tree content may have changed.
        if self.selected.borrow().is_some() {
            self.load_diff();
        }

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

    fn load_diff(self: &Rc<Self>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let Some((side, files)) = self.selected.borrow().clone() else { return };
        let generation = self.diff_gen.get() + 1;
        self.diff_gen.set(generation);
        let git = rv.git.clone();
        let opts = self.diff.options();
        let this = self.clone();
        let files: Vec<FileItem> = files.into_iter().take(50).collect();
        let all_conflicted = side == Side::Unstaged && files.iter().all(|f| f.conflicted);
        spawn(async move {
            let g2 = git.clone();
            let result = bg(move || {
                let mut out = Vec::new();
                for f in &files {
                    let r = match side {
                        Side::Staged => diff::staged(&g2, &f.path, opts),
                        Side::Unstaged if f.conflicted => Ok(conflict_view(&g2, &f.path)),
                        Side::Unstaged if f.untracked => diff::untracked(&g2, &f.path, opts),
                        Side::Unstaged => diff::unstaged(&g2, &f.path, opts),
                    };
                    match r {
                        Ok(mut v) => {
                            if v.is_empty() {
                                // e.g. a deleted/renamed file only visible with other options
                                v.push(FileDiff {
                                    new_path: Some(f.path.clone()),
                                    ..Default::default()
                                });
                            }
                            out.extend(v)
                        }
                        Err(e) => return Err(e),
                    }
                }
                Ok(out)
            })
            .await;
            if this.diff_gen.get() != generation {
                return;
            }
            match result {
                Ok(files) => {
                    let (kind, old, new) = match side {
                        Side::Staged => (DiffKind::Staged, Some("HEAD".to_string()), Some(String::new())),
                        Side::Unstaged if all_conflicted => (DiffKind::Conflict, Some(String::new()), None),
                        Side::Unstaged => (DiffKind::Unstaged, Some(String::new()), None),
                    };
                    this.diff.show(
                        files,
                        kind,
                        Some(DiffContext {
                            git,
                            old_rev: old,
                            new_rev: new,
                        }),
                    );
                }
                Err(e) => this.diff.show_message(&e.to_string()),
            }
        });
    }

    fn apply_patch_action(self: &Rc<Self>, action: PatchAction, file: FileDiff, sel: Option<diff::Selection>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let path = file.path().to_string();
        let item = self
            .selected
            .borrow()
            .as_ref()
            .and_then(|(_, fs)| fs.iter().find(|f| f.path == path).cloned());
        let untracked = item.as_ref().is_some_and(|f| f.untracked);
        let conflicted = item.as_ref().is_some_and(|f| f.conflicted);
        let Some(sel) = sel.filter(|_| !conflicted) else {
            // Whole-file operations.
            match action {
                PatchAction::Stage => self.stage_paths(vec![path]),
                PatchAction::Unstage => self.unstage_paths(vec![path]),
                PatchAction::Discard => {
                    super::dialogs::dispatch(&rv, "file-discard", encode_target(Side::Unstaged, &[path]));
                }
            }
            return;
        };
        if self.diff.options().ignore_whitespace {
            super::show_error(
                &self.widget,
                "Cannot stage partial changes",
                "Turn off “Ignore whitespace” to stage, unstage or discard individual hunks or lines.",
            );
            return;
        }
        let this = self.clone();
        spawn(async move {
            if action == PatchAction::Discard
                && !super::confirm(
                    &this.widget,
                    "Discard Changes?",
                    "The selected changes will be permanently lost.",
                    "Discard",
                    true,
                )
                .await
            {
                return;
            }
            let git = rv.git.clone();
            let opts = this.diff.options();
            let r = bg(move || {
                let target = match action {
                    PatchAction::Stage => diff::ApplyTarget::Stage,
                    PatchAction::Unstage => diff::ApplyTarget::Unstage,
                    PatchAction::Discard => diff::ApplyTarget::Discard,
                };
                diff::apply_selection(&git, &file, &sel, target, untracked, opts)
            })
            .await;
            if let Err(e) = r {
                super::show_error(&this.widget, "Could not apply the change", &e.to_string());
            }
            rv.refresh();
        });
    }

    pub fn stage_paths(self: &Rc<Self>, paths: Vec<String>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let mut cmd = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
        cmd.extend(paths);
        quick(&rv, cmd);
    }

    pub fn unstage_paths(self: &Rc<Self>, paths: Vec<String>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let has_head = rv.snapshot().refs.head_oid.is_some();
        let mut cmd: Vec<String> = if has_head {
            vec!["reset".into(), "-q".into(), "HEAD".into(), "--".into()]
        } else {
            vec!["rm".into(), "--cached".into(), "-r".into(), "-q".into(), "--".into()]
        };
        cmd.extend(paths);
        quick(&rv, cmd);
    }

    fn context_menu(&self, widget: &gtk::Widget, x: f64, y: f64, side: Side, files: &[FileItem]) {
        if files.is_empty() {
            return;
        }
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let t = encode_target(side, &paths);
        let single = files.len() == 1;
        let menu = gio::Menu::new();

        let s1 = gio::Menu::new();
        match side {
            Side::Unstaged => menu_item_target(&s1, "Stage", "repo.file-stage", &t),
            Side::Staged => menu_item_target(&s1, "Unstage", "repo.file-unstage", &t),
        }
        menu_item_target(&s1, "Discard Changes…", "repo.file-discard", &t);
        menu_item_target(&s1, "Remove…", "repo.file-remove", &t);
        if files.iter().all(|f| !f.untracked) {
            menu_item_target(&s1, "Stop Tracking", "repo.file-stop-tracking", &t);
        }
        if files.iter().any(|f| f.untracked) {
            menu_item_target(&s1, "Ignore…", "repo.file-ignore", &t);
        }
        menu.append_section(None, &s1);

        if files.iter().any(|f| f.conflicted) {
            let s = gio::Menu::new();
            menu_item_target(&s, "Resolve Using “Mine”", "repo.file-resolve-mine", &t);
            menu_item_target(&s, "Resolve Using “Theirs”", "repo.file-resolve-theirs", &t);
            menu_item_target(&s, "Launch External Merge Tool", "repo.file-mergetool", &t);
            menu_item_target(&s, "Mark Resolved", "repo.file-mark-resolved", &t);
            menu_item_target(&s, "Restart Merge (Mark Unresolved)", "repo.file-mark-unresolved", &t);
            menu.append_submenu(Some("Resolve Conflicts"), &s);
        }

        let s2 = gio::Menu::new();
        if single {
            menu_item_target(&s2, "Open", "repo.file-open", &paths[0]);
            menu_item_target(&s2, "Show in Files", "repo.file-show", &paths[0]);
            menu_item_target(&s2, "External Diff", "repo.file-difftool", &t);
        }
        menu_item_target(&s2, "Copy Path", "repo.copy-text", &paths.join("\n"));
        menu.append_section(None, &s2);

        if single && !files[0].untracked {
            let s3 = gio::Menu::new();
            menu_item_target(&s3, "Log Selected…", "repo.file-log", &paths[0]);
            menu_item_target(&s3, "Blame Selected…", "repo.file-blame", &paths[0]);
            menu.append_section(None, &s3);
        }
        let actions = config::with(|s| s.custom_actions.clone());
        if !actions.is_empty() {
            let s4 = gio::Menu::new();
            for (i, a) in actions.iter().enumerate() {
                menu_item_target(&s4, &a.name, "repo.custom-action", &format!("{i}||{}", paths[0]));
            }
            menu.append_submenu(Some("Custom Actions"), &s4);
        }
        popup_menu(widget, x, y, &menu);
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

/// Runs a quick local command (no progress sheet unless it's slow).
fn quick(rv: &Rc<RepoView>, cmd: Vec<String>) {
    let rv = rv.clone();
    spawn(async move {
        rv.run_ops(&crate::git::describe(&cmd), vec![cmd], OpOptions::default())
            .await;
    });
}

/// Shows the working tree file of a conflicted path as a single hunk.
fn conflict_view(git: &Git, path: &str) -> Vec<FileDiff> {
    let content = std::fs::read_to_string(git.workdir.join(path)).unwrap_or_default();
    let lines: Vec<crate::git::diff::DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let kind = if l.starts_with("<<<<<<<") || l.starts_with(">>>>>>>") || l.starts_with("=======") || l.starts_with("|||||||") {
                LineKind::Del
            } else {
                LineKind::Context
            };
            crate::git::diff::DiffLine {
                kind,
                text: l.to_string(),
                old_no: Some(i as u32 + 1),
                new_no: Some(i as u32 + 1),
            }
        })
        .collect();
    let n = lines.len() as u32;
    vec![FileDiff {
        old_path: Some(path.to_string()),
        new_path: Some(path.to_string()),
        header: vec![],
        hunks: vec![Hunk {
            old_start: 1,
            old_count: n,
            new_start: 1,
            new_count: n,
            header: "@@ conflicted file @@ Resolve with the context menu or edit the file".into(),
            lines,
        }],
        ..Default::default()
    }]
}
