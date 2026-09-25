//! One repository tab: toolbar, sidebar, and the File Status / History /
//! Search views.

use super::file_status::FileStatusView;
use super::history::{HistoryMode, HistoryView};
use super::panes::Keep;
use super::progress::{self, OpOptions};
use super::sidebar::Sidebar;
use super::{bg, spawn};
use crate::config;
use crate::git::flow::FlowConfig;
use crate::git::refs::{Refs, Remote, Stash, Submodule, Subtree};
use crate::git::state::OpState;
use crate::git::status::Status;
use crate::git::{self, Git, GitError};
use crate::watch;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub refs: Refs,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<Remote>,
    pub submodules: Vec<Submodule>,
    pub subtrees: Vec<Subtree>,
    pub op: OpState,
    pub flow: Option<FlowConfig>,
}

impl Snapshot {
    pub fn current_branch(&self) -> Option<&str> {
        self.refs.head_branch.as_deref()
    }

    /// Remote used by default for pull/push.
    pub fn default_remote(&self) -> Option<String> {
        if let Some(up) = self.refs.current().and_then(|r| r.upstream.clone())
            && let Some((remote, _)) = up.split_once('/')
                && self.remotes.iter().any(|r| r.name == remote) {
                    return Some(remote.to_string());
                }
        self.remotes
            .iter()
            .find(|r| r.name == "origin")
            .or(self.remotes.first())
            .map(|r| r.name.clone())
    }

    pub fn has_changes(&self) -> bool {
        self.status.entries.iter().any(|e| !e.ignored)
    }
}

fn load_snapshot(git: &Git, git_dir: &Path, include_ignored: bool) -> Result<Snapshot, GitError> {
    let status = git::status::status(git, include_ignored)?;
    let refs = git::refs::refs(git)?;
    let stashes = git::refs::stashes(git).unwrap_or_default();
    let remotes = git::refs::remotes(git).unwrap_or_default();
    let submodules = git::refs::submodules(git);
    let subtrees = git::refs::subtrees(git);
    let op = git::state::op_state(git_dir);
    let flow = git::flow::config(git);
    Ok(Snapshot {
        status,
        refs,
        stashes,
        remotes,
        submodules,
        subtrees,
        op,
        flow,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    History,
    Search,
}

pub struct RepoView {
    pub widget: gtk::Box,
    pub git: Git,
    pub git_dir: PathBuf,
    pub name: String,
    snap: RefCell<Rc<Snapshot>>,
    stack: gtk::Stack,
    pub sidebar: Rc<Sidebar>,
    pub file_status: Rc<FileStatusView>,
    pub history: Rc<HistoryView>,
    pub search: Rc<HistoryView>,
    banner: gtk::Revealer,
    banner_label: gtk::Label,
    banner_skip: gtk::Button,
    banner_continue: gtk::Button,
    error_banner: adw::Banner,
    pull_badge: gtk::Label,
    push_badge: gtk::Label,
    busy: Cell<bool>,
    refreshing: Cell<bool>,
    refresh_pending: Cell<bool>,
    watcher: RefCell<Option<watch::Watcher>>,
    pub open_repo: Rc<dyn Fn(PathBuf)>,
    last_log_key: RefCell<String>,
    shown_once: Cell<bool>,
    pub include_ignored: Cell<bool>,
    pub actions: gio::SimpleActionGroup,
}

impl RepoView {
    pub fn new(workdir: PathBuf, open_repo: Rc<dyn Fn(PathBuf)>) -> Rc<Self> {
        let git = Git::new(&workdir);
        let git_dir = git.git_dir();
        let name = workdir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| workdir.to_string_lossy().to_string());
        let name = config::with(|s| {
            s.bookmarks
                .iter()
                .find(|b| b.path == workdir)
                .map(|b| b.name.clone())
        })
        .unwrap_or(name);

        let rv = Rc::new_cyclic(|weak: &Weak<RepoView>| {
            let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let (toolbar, pull_badge, push_badge) = build_toolbar();
            widget.append(&toolbar);

            let error_banner = adw::Banner::new("");
            widget.append(&error_banner);

            // In-progress operation banner (merge / rebase / cherry-pick).
            let banner_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            banner_box.add_css_class("op-banner");
            let banner_label = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
            let banner_continue = gtk::Button::with_label("Continue");
            banner_continue.add_css_class("suggested-action");
            banner_continue.set_action_name(Some("repo.op-continue"));
            banner_continue.set_action_target_value(Some(&"".to_variant()));
            let banner_skip = gtk::Button::with_label("Skip");
            banner_skip.set_action_name(Some("repo.op-skip"));
            banner_skip.set_action_target_value(Some(&"".to_variant()));
            let banner_abort = gtk::Button::with_label("Abort");
            banner_abort.add_css_class("destructive-action");
            banner_abort.set_action_name(Some("repo.op-abort"));
            banner_abort.set_action_target_value(Some(&"".to_variant()));
            banner_box.append(&banner_label);
            banner_box.append(&banner_skip);
            banner_box.append(&banner_abort);
            banner_box.append(&banner_continue);
            let banner = gtk::Revealer::builder().child(&banner_box).reveal_child(false).build();
            widget.append(&banner);

            let sidebar = Sidebar::new(weak.clone());
            let file_status = FileStatusView::new(weak.clone());
            let history = HistoryView::with_git(weak.clone(), git.clone(), HistoryMode::History);
            let search = HistoryView::with_git(weak.clone(), git.clone(), HistoryMode::Search);

            let stack = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .transition_duration(100)
                .hexpand(true)
                .build();
            stack.add_named(&file_status.widget, Some("status"));
            stack.add_named(&history.widget, Some("history"));
            stack.add_named(&search.widget, Some("search"));

            let paned = gtk::Paned::builder()
                .orientation(gtk::Orientation::Horizontal)
                .start_child(&sidebar.widget)
                .end_child(&stack)
                .shrink_start_child(false)
                .vexpand(true)
                .build();
            super::panes::remember(&paned, "sidebar", Keep::Start, 230);
            let toast = adw::ToastOverlay::new();
            toast.set_child(Some(&paned));
            widget.append(&toast);

            RepoView {
                widget,
                git,
                git_dir,
                name,
                snap: RefCell::new(Rc::new(Snapshot::default())),
                stack,
                sidebar,
                file_status,
                history,
                search,
                banner,
                banner_label,
                banner_skip,
                banner_continue,
                error_banner,
                pull_badge,
                push_badge,
                busy: Cell::new(false),
                refreshing: Cell::new(false),
                refresh_pending: Cell::new(false),
                watcher: RefCell::new(None),
                open_repo,
                last_log_key: RefCell::new(String::new()),
                shown_once: Cell::new(false),
                include_ignored: Cell::new(false),
                actions: gio::SimpleActionGroup::new(),
            }
        });
        rv.setup_actions();
        rv.show_view(View::History);
        rv
    }

    pub fn snapshot(&self) -> Rc<Snapshot> {
        self.snap.borrow().clone()
    }

    pub fn weak(self: &Rc<Self>) -> Weak<RepoView> {
        Rc::downgrade(self)
    }

    /// Called whenever the tab becomes visible.
    pub fn on_shown(self: &Rc<Self>) {
        if !self.shown_once.replace(true) {
            self.start_watching();
            self.start_fetch_timer();
        }
        self.refresh();
    }

    fn start_watching(self: &Rc<Self>) {
        let (tx, rx) = async_channel::unbounded::<Vec<watch::Change>>();
        let w = watch::watch(self.git.workdir.clone(), self.git_dir.clone(), move |kinds| {
            let _ = tx.send_blocking(kinds);
        });
        *self.watcher.borrow_mut() = w;
        let weak = self.weak();
        spawn(async move {
            while let Ok(_kinds) = rx.recv().await {
                // Coalesce bursts.
                while rx.try_recv().is_ok() {}
                match weak.upgrade() {
                    Some(rv) => rv.refresh(),
                    None => break,
                }
            }
        });
    }

    fn start_fetch_timer(self: &Rc<Self>) {
        let weak = self.weak();
        glib::timeout_add_seconds_local(60, move || {
            let Some(rv) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let interval = config::with(|s| s.fetch_interval_min) as u64;
            thread_local! {
                static LAST: RefCell<std::collections::HashMap<PathBuf, std::time::Instant>> = RefCell::new(Default::default());
            }
            if interval == 0 || rv.busy.get() || rv.snapshot().remotes.is_empty() {
                return glib::ControlFlow::Continue;
            }
            let due = LAST.with(|l| {
                let mut l = l.borrow_mut();
                let now = std::time::Instant::now();
                let last = l.entry(rv.git.workdir.clone()).or_insert(now);
                if now.duration_since(*last) >= Duration::from_secs(interval * 60) {
                    *last = now;
                    true
                } else {
                    false
                }
            });
            if due {
                let rv2 = rv.clone();
                spawn(async move {
                    let _ = progress::run(
                        &rv2.widget.clone().upcast(),
                        &rv2.git,
                        "Background fetch",
                        vec![vec!["fetch".into(), "--all".into(), "--quiet".into()]],
                        OpOptions {
                            quiet: true,
                            ..Default::default()
                        },
                    )
                    .await;
                    rv2.refresh();
                });
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn refresh(self: &Rc<Self>) {
        if self.refreshing.get() || self.busy.get() {
            self.refresh_pending.set(true);
            return;
        }
        self.refreshing.set(true);
        if std::env::var_os("GITREE_DEBUG_REFRESH").is_some() {
            eprintln!("gitree: refresh {}", self.name);
        }
        let this = self.clone();
        spawn(async move {
            let git = this.git.clone();
            let gd = this.git_dir.clone();
            let ign = this.include_ignored.get();
            let r = bg(move || load_snapshot(&git, &gd, ign)).await;
            this.refreshing.set(false);
            match r {
                Ok(s) => {
                    this.error_banner.set_revealed(false);
                    this.apply_snapshot(s);
                }
                Err(e) => {
                    this.error_banner.set_title(&format!(
                        "Could not read repository: {}",
                        e.stderr.lines().next().unwrap_or("")
                    ));
                    this.error_banner.set_revealed(true);
                }
            }
            if this.refresh_pending.replace(false) {
                this.refresh();
            }
        });
    }

    fn apply_snapshot(self: &Rc<Self>, s: Snapshot) {
        let s = Rc::new(s);
        *self.snap.borrow_mut() = s.clone();

        let (ahead, behind) = s
            .refs
            .current()
            .map(|r| (r.ahead, r.behind))
            .unwrap_or((0, 0));
        super::set_badge(&self.push_badge, ahead as usize);
        super::set_badge(&self.pull_badge, behind as usize);

        if s.op.is_none() {
            self.banner.set_reveal_child(false);
        } else {
            let mut text = s.op.label();
            if s.status.has_conflicts() {
                text.push_str(" Resolve the conflicted files, then continue.");
            }
            self.banner_label.set_text(&text);
            self.banner_skip.set_visible(matches!(
                s.op,
                OpState::Rebase { .. } | OpState::CherryPick | OpState::Revert
            ));
            self.banner_continue.set_visible(!matches!(s.op, OpState::Bisect));
            self.banner.set_reveal_child(true);
        }

        self.sidebar.update(&s);
        self.file_status.update(&s);
        self.history.details.update_uncommitted(&s);

        // Reload history only when refs or the dirty state changed.
        let key = format!("{}|{}", s.refs.signature(), s.has_changes());
        if *self.last_log_key.borrow() != key {
            *self.last_log_key.borrow_mut() = key;
            self.history.reload();
        }
    }

    /// Forces the commit list to be reloaded on next refresh.
    pub fn invalidate_log(&self) {
        self.last_log_key.borrow_mut().clear();
    }

    pub fn show_view(&self, v: View) {
        self.stack.set_visible_child_name(match v {
            View::Status => "status",
            View::History => "history",
            View::Search => "search",
        });
        self.sidebar.select_view(v);
        if v == View::Search {
            self.search.focus_search();
        }
    }

    /// Selects a commit in the History view.
    pub fn jump_to_commit(self: &Rc<Self>, oid: &str) {
        self.show_view(View::History);
        self.history.select_commit(oid);
    }

    pub fn toast(&self, msg: &str) {
        super::toast(&self.stack, msg);
    }

    /// Runs git commands with the progress sheet, then refreshes.
    pub async fn run_ops(self: &Rc<Self>, title: &str, cmds: Vec<Vec<String>>, opts: OpOptions) -> bool {
        self.busy.set(true);
        let r = progress::run(&self.widget.clone().upcast(), &self.git, title, cmds, opts).await;
        self.busy.set(false);
        self.refresh_pending.set(false);
        self.refresh();
        r.is_ok()
    }

    fn setup_actions(self: &Rc<Self>) {
        let group = self.actions.clone();
        for name in super::dialogs::ACTIONS {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let weak = self.weak();
            let n = name.to_string();
            a.connect_activate(move |_, v| {
                if let Some(rv) = weak.upgrade() {
                    let arg = v.and_then(|v| v.get::<String>()).unwrap_or_default();
                    super::dialogs::dispatch(&rv, &n, arg);
                }
            });
            group.add_action(&a);
        }
        self.widget.insert_action_group("repo", Some(&group));
    }
}

fn tool_button(icon: &str, label: &str, action: &str, tooltip: &str) -> (gtk::Button, gtk::Label) {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(20);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&img));
    let badge = super::count_badge();
    badge.set_halign(gtk::Align::End);
    badge.set_valign(gtk::Align::Start);
    badge.set_margin_end(0);
    overlay.add_overlay(&badge);
    vbox.append(&overlay);
    let l = gtk::Label::new(Some(label));
    l.add_css_class("caption");
    vbox.append(&l);
    let b = gtk::Button::builder()
        .child(&vbox)
        .action_name(action)
        .action_target(&"".to_variant())
        .tooltip_text(tooltip)
        .build();
    b.add_css_class("flat");
    b.add_css_class("tool");
    (b, badge)
}

fn build_toolbar() -> (gtk::Box, gtk::Label, gtk::Label) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    bar.add_css_class("repo-toolbar");
    let sep = || gtk::Separator::new(gtk::Orientation::Vertical);

    let (commit, _) = tool_button("gitree-commit-symbolic", "Commit", "repo.commit", "Commit (Ctrl+Shift+C)");
    bar.append(&commit);
    bar.append(&sep());
    let (pull, pull_badge) = tool_button("gitree-pull-symbolic", "Pull", "repo.pull", "Pull (Ctrl+Shift+L)");
    let (push, push_badge) = tool_button("gitree-push-symbolic", "Push", "repo.push", "Push (Ctrl+Shift+P)");
    let (fetch, _) = tool_button("gitree-fetch-symbolic", "Fetch", "repo.fetch", "Fetch (Ctrl+Shift+F)");
    bar.append(&pull);
    bar.append(&push);
    bar.append(&fetch);
    bar.append(&sep());
    let (branch, _) = tool_button("gitree-branch-symbolic", "Branch", "repo.branch", "Branch (Ctrl+Shift+B)");
    let (merge, _) = tool_button("gitree-merge-symbolic", "Merge", "repo.merge", "Merge (Ctrl+Shift+M)");
    bar.append(&branch);
    bar.append(&merge);
    bar.append(&sep());
    let (stash, _) = tool_button("gitree-stash-symbolic", "Stash", "repo.stash", "Stash (Ctrl+Shift+S)");
    let (discard, _) = tool_button("gitree-discard-symbolic", "Discard", "repo.discard", "Discard (Ctrl+Shift+R)");
    let (tag, _) = tool_button("gitree-tag-symbolic", "Tag", "repo.tag", "Tag (Ctrl+Shift+T)");
    bar.append(&stash);
    bar.append(&discard);
    bar.append(&tag);
    bar.append(&sep());
    let (flow, _) = tool_button("gitree-flow-symbolic", "Git-flow", "repo.flow", "Git-flow");
    bar.append(&flow);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let (remote, _) = tool_button("gitree-remote-symbolic", "Remote", "repo.open-remote", "Open remote in web browser");
    let (term, _) = tool_button("gitree-terminal-symbolic", "Terminal", "repo.terminal", "Open in Terminal (Ctrl+Alt+T)");
    let (files, _) = tool_button("folder-open-symbolic", "Files", "repo.files", "Show in Files (Ctrl+Alt+O)");
    bar.append(&remote);
    bar.append(&term);
    bar.append(&files);

    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    super::menu_item_target(&s1, "Repository Settings…", "repo.settings", "");
    super::menu_item_target(&s1, "Refresh", "repo.refresh", "");
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    super::menu_item_target(&s2, "Git LFS…", "repo.lfs", "");
    super::menu_item_target(&s2, "Add Submodule…", "repo.add-submodule", "");
    super::menu_item_target(&s2, "Add / Link Subtree…", "repo.add-subtree", "");
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    super::menu_item_target(&s3, "Apply Patch…", "repo.apply-patch", "");
    super::menu_item_target(&s3, "Interactive Rebase…", "repo.rebase-interactive-pick", "");
    super::menu_item_target(&s3, "Cleanup Untracked Files…", "repo.clean", "");
    super::menu_item_target(&s3, "Garbage Collect", "repo.gc", "");
    menu.append_section(None, &s3);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let img = gtk::Image::from_icon_name("preferences-system-symbolic");
    img.set_pixel_size(20);
    vbox.append(&img);
    let l = gtk::Label::new(Some("Settings"));
    l.add_css_class("caption");
    vbox.append(&l);
    let more = gtk::MenuButton::builder()
        .child(&vbox)
        .menu_model(&menu)
        .tooltip_text("Repository settings and more actions")
        .always_show_arrow(false)
        .build();
    more.add_css_class("flat");
    more.add_css_class("tool");
    bar.append(&more);

    let sw_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&bar)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hscrollbar_policy(gtk::PolicyType::External)
        .hexpand(true)
        .build();
    sw_bar.append(&scrolled);
    (sw_bar, pull_badge, push_badge)
}
