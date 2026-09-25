//! Staged / unstaged file lists and a diff that stages, unstages and
//! discards whole files, hunks or lines. Used by the File Status view and by
//! the uncommitted changes row in History.

use super::diff_view::{DiffContext, DiffKind, DiffView, PatchAction};
use super::file_list::{FileItem, FileList};
use super::panes::{self, Keep};
use super::progress::OpOptions;
use super::repo_view::{RepoView, Snapshot};
use super::{bg, menu_item_target, popup_menu, spawn};
use crate::config;
use crate::git::diff::{self, FileDiff, Hunk, LineKind};
use crate::git::status::StatusEntry;
use crate::git::Git;
use adw::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
}

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

pub struct StagingView {
    /// The staged list above the unstaged list.
    pub lists: gtk::Paned,
    pub diff: Rc<DiffView>,
    rv: Weak<RepoView>,
    staged: Rc<FileList>,
    unstaged: Rc<FileList>,
    staged_pane: gtk::Box,
    staged_count: gtk::Label,
    unstaged_count: gtk::Label,
    selected: RefCell<Option<(Side, Vec<FileItem>)>>,
    diff_gen: Cell<u64>,
    /// Hide the staged list while nothing is staged.
    collapse_staged: bool,
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

impl StagingView {
    /// `staged_height` is the default height of the staged list, whose
    /// size is remembered under `pane_key`.
    pub fn new(rv: Weak<RepoView>, pane_key: &'static str, staged_height: i32, collapse_staged: bool) -> Rc<Self> {
        let staged = FileList::new(Some(true));
        let unstaged = FileList::new(Some(false));
        let unstage_all = gtk::Button::with_label("Unstage All");
        let unstage_sel = gtk::Button::with_label("Unstage Selected");
        let stage_all = gtk::Button::with_label("Stage All");
        let stage_sel = gtk::Button::with_label("Stage Selected");
        let (staged_pane, staged_count) = pane("Staged files", &staged, &[&unstage_all, &unstage_sel]);
        let (unstaged_pane, unstaged_count) = pane("Unstaged files", &unstaged, &[&stage_all, &stage_sel]);
        staged_pane.set_visible(!collapse_staged);

        let lists = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&staged_pane)
            .end_child(&unstaged_pane)
            .vexpand(true)
            .build();
        panes::remember(&lists, pane_key, Keep::Start, staged_height);

        let this = Rc::new(Self {
            lists,
            diff: DiffView::new(),
            rv,
            staged,
            unstaged,
            staged_pane,
            staged_count,
            unstaged_count,
            selected: RefCell::new(None),
            diff_gen: Cell::new(0),
            collapse_staged,
        });
        this.set_tree_mode(config::with(|s| s.file_tree_view));
        this.connect_lists();
        this.connect_buttons(&stage_all, &stage_sel, &unstage_all, &unstage_sel);
        this.connect_diff();
        this
    }

    pub fn set_tree_mode(&self, tree: bool) {
        self.staged.set_tree_mode(tree);
        self.unstaged.set_tree_mode(tree);
    }

    /// Fills the lists from the status entries accepted by `keep`, which
    /// gets the entry and the status letter shown for that list.
    pub fn update(self: &Rc<Self>, s: &Snapshot, keep: impl Fn(&StatusEntry, char) -> bool) {
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        for e in &s.status.entries {
            let item_for = |code: char| FileItem {
                path: e.path.clone(),
                orig_path: e.orig_path.clone(),
                code,
                untracked: e.untracked,
                conflicted: e.conflicted,
            };
            if e.is_staged() && keep(e, e.staged_code()) {
                staged.push(item_for(e.staged_code()));
            }
            if e.is_unstaged() && keep(e, e.unstaged_code()) {
                unstaged.push(item_for(e.unstaged_code()));
            }
        }
        let count = |v: &Vec<FileItem>| if v.is_empty() { String::new() } else { v.len().to_string() };
        self.staged_count.set_text(&count(&staged));
        self.unstaged_count.set_text(&count(&unstaged));
        if self.collapse_staged {
            self.staged_pane.set_visible(!staged.is_empty());
        }
        self.staged.set_files(staged);
        self.unstaged.set_files(unstaged);

        // Refresh the diff: working tree content may have changed.
        if self.selected.borrow().is_some() {
            self.load_diff();
        }
    }

    /// Selects the first file, preferring the unstaged list.
    pub fn select_first(&self) {
        if !self.unstaged.all_files().is_empty() {
            self.unstaged.select_first();
        } else if !self.staged.all_files().is_empty() {
            self.staged.select_first();
        } else {
            self.diff.show_message("No file changes");
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selected.borrow().is_some()
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

    pub fn debug_select(&self, staged: bool, index: u32) {
        let list = if staged { &self.staged } else { &self.unstaged };
        list.selection.select_item(index, true);
        // Scroll to and focus the row once the list has been laid out.
        let view = list.list.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            view.scroll_to(index, gtk::ListScrollFlags::FOCUS, None);
            view.grab_focus();
        });
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
                &self.lists,
                "Cannot stage partial changes",
                "Turn off “Ignore whitespace” to stage, unstage or discard individual hunks or lines.",
            );
            return;
        }
        let this = self.clone();
        spawn(async move {
            if action == PatchAction::Discard
                && !super::confirm(
                    &this.lists,
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
                super::show_error(&this.lists, "Could not apply the change", &e.to_string());
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
