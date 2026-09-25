//! History (commit graph), Search and File Log views, plus the commit
//! details pane shared by all of them.

use super::diff_view::{DiffContext, DiffKind, DiffView};
use super::file_list::{FileItem, FileList};
use super::graph_cell::{self, NodeStyle};
use super::repo_view::{RepoView, Snapshot, View};
use super::staging::StagingView;
use super::{bg, format_time, format_time_full, menu_item_target, popup_menu, spawn};
use crate::config;
use crate::git::diff;
use crate::git::graph::{GraphBuilder, GraphRow};
use crate::git::log::{Commit, LogOrder, LogQuery, LogReader, LogScope};
use crate::git::refs::{RefInfo, RefKind};
use crate::git::Git;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMode {
    History,
    Search,
    FileLog(String),
}

#[derive(Debug, Clone)]
pub struct LogRow {
    pub commit: Commit,
    pub graph: GraphRow,
    pub refs: Vec<RefInfo>,
    pub is_head: bool,
    pub uncommitted: bool,
}

fn row_of(obj: &glib::Object) -> Option<Rc<LogRow>> {
    let b = obj.downcast_ref::<glib::BoxedAnyObject>()?;
    let r = b.borrow::<Rc<LogRow>>().clone();
    Some(r)
}

// ---------------------------------------------------------------------------
// Commit details pane

pub struct CommitDetails {
    /// Shows `commit_pane`, or the staging lists for uncommitted changes.
    pub widget: gtk::Stack,
    commit_pane: gtk::Paned,
    /// Created the first time the uncommitted changes are selected.
    staging: RefCell<Option<Rc<StagingView>>>,
    info: gtk::Box,
    files: Rc<FileList>,
    pub diff: Rc<DiffView>,
    rv: Weak<RepoView>,
    git: Git,
    /// (from, to) — from is None for a single commit (diff to first parent).
    target: RefCell<Option<(Option<String>, String)>>,
    path_filter: Option<String>,
    generation: Cell<u64>,
}

impl CommitDetails {
    pub fn new(rv: Weak<RepoView>, git: Git, path_filter: Option<String>) -> Rc<Self> {
        let info = gtk::Box::new(gtk::Orientation::Vertical, 4);
        info.set_margin_top(8);
        info.set_margin_bottom(8);
        info.set_margin_start(10);
        info.set_margin_end(10);
        let info_sw = gtk::ScrolledWindow::builder()
            .child(&info)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(260)
            .build();
        let files = FileList::new(None);
        let left = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&info_sw)
            .end_child(&files.widget)
            .position(170)
            .build();
        let diff = DiffView::new();
        let commit_pane = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&left)
            .end_child(&diff.widget)
            .position(420)
            .shrink_start_child(false)
            .build();
        let widget = gtk::Stack::new();
        widget.add_named(&commit_pane, Some("commit"));
        let this = Rc::new(Self {
            widget,
            commit_pane,
            staging: RefCell::new(None),
            info,
            files,
            diff,
            rv,
            git,
            target: RefCell::new(None),
            path_filter,
            generation: Cell::new(0),
        });
        let w = Rc::downgrade(&this);
        *this.files.on_selection.borrow_mut() = Some(Box::new(move |files| {
            if let Some(t) = w.upgrade() {
                t.load_diff(files);
            }
        }));
        let w = Rc::downgrade(&this);
        *this.diff.on_options_changed.borrow_mut() = Some(Box::new(move |()| {
            if let Some(t) = w.upgrade() {
                t.load_diff(t.files.selected_files());
            }
        }));
        let w = Rc::downgrade(&this);
        *this.diff.on_external.borrow_mut() = Some(Box::new(move |path| {
            if let Some(t) = w.upgrade()
                && let (Some(rv), Some((from, to))) = (t.rv.upgrade(), t.target.borrow().clone()) {
                    let from = from.unwrap_or_else(|| format!("{to}^"));
                    super::dialogs::external_diff(&rv, &path, false, Some((from, to)));
                }
        }));
        let w = Rc::downgrade(&this);
        *this.files.on_context.borrow_mut() = Some(Box::new(move |widget, x, y, files| {
            let Some(t) = w.upgrade() else { return };
            let Some((_, to)) = t.target.borrow().clone() else { return };
            if files.len() != 1 {
                return;
            }
            let p = &files[0].path;
            let menu = gio::Menu::new();
            let s1 = gio::Menu::new();
            menu_item_target(&s1, "Open Current Version", "repo.file-open", p);
            menu_item_target(&s1, "Show in Files", "repo.file-show", p);
            menu_item_target(&s1, "Copy Path", "repo.copy-text", p);
            menu.append_section(None, &s1);
            let s2 = gio::Menu::new();
            menu_item_target(&s2, "Log Selected…", "repo.file-log", p);
            menu_item_target(&s2, "Blame Selected…", "repo.file-blame", p);
            menu_item_target(&s2, "Blame at This Commit…", "repo.file-blame-at", &format!("{to}|{p}"));
            menu_item_target(&s2, "Reset File to This Commit…", "repo.file-checkout-at", &format!("{to}|{p}"));
            menu_item_target(&s2, "Reset File to Parent Commit…", "repo.file-checkout-at", &format!("{to}^|{p}"));
            menu.append_section(None, &s2);
            popup_menu(widget, x, y, &menu);
        }));
        this.clear();
        this
    }

    pub fn clear(&self) {
        self.widget.set_visible_child_name("commit");
        while let Some(c) = self.info.first_child() {
            self.info.remove(&c);
        }
        self.files.set_files(Vec::new());
        self.diff.show_message("No commit selected");
        *self.target.borrow_mut() = None;
    }

    fn info_row(&self, key: &str, value: &gtk::Widget) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let k = gtk::Label::builder()
            .label(key)
            .xalign(1.0)
            .width_chars(10)
            .valign(gtk::Align::Start)
            .build();
        k.add_css_class("dim-label");
        row.append(&k);
        row.append(value);
        self.info.append(&row);
    }

    fn value_label(text: &str) -> gtk::Label {
        
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .hexpand(true)
            .build()
    }

    /// Shows a single commit.
    pub fn show_commit(self: &Rc<Self>, c: &Commit, refs: &[RefInfo]) {
        self.widget.set_visible_child_name("commit");
        while let Some(ch) = self.info.first_child() {
            self.info.remove(&ch);
        }
        let sha = Self::value_label(&c.oid);
        sha.add_css_class("mono");
        self.info_row("Commit:", sha.upcast_ref());

        let parents = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for p in &c.parents {
            let b = gtk::LinkButton::with_label("", &p[..p.len().min(10)]);
            b.set_tooltip_text(Some(p));
            let rv = self.rv.clone();
            let oid = p.clone();
            b.connect_activate_link(move |_| {
                if let Some(rv) = rv.upgrade() {
                    rv.jump_to_commit(&oid);
                }
                glib::Propagation::Stop
            });
            parents.append(&b);
        }
        if !c.parents.is_empty() {
            self.info_row(if c.parents.len() > 1 { "Parents:" } else { "Parent:" }, parents.upcast_ref());
        }
        self.info_row(
            "Author:",
            Self::value_label(&format!("{} <{}>", c.author_name, c.author_email)).upcast_ref(),
        );
        self.info_row("Date:", Self::value_label(&format_time_full(c.author_time)).upcast_ref());
        if c.committer_email != c.author_email || c.committer_name != c.author_name {
            self.info_row(
                "Committer:",
                Self::value_label(&format!("{} <{}>", c.committer_name, c.committer_email)).upcast_ref(),
            );
        }
        if !refs.is_empty() {
            let names: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
            self.info_row("Labels:", Self::value_label(&names.join(", ")).upcast_ref());
        }
        let msg = Self::value_label(&c.subject);
        msg.set_margin_top(8);
        msg.add_css_class("heading");
        self.info.append(&msg);
        let body = Self::value_label("");
        body.set_visible(false);
        self.info.append(&body);
        // Full message (body) is loaded lazily.
        let git = self.git.clone();
        let oid = c.oid.clone();
        let subject = c.subject.clone();
        spawn(async move {
            if let Ok(m) = bg(move || crate::git::log::message(&git, &oid)).await {
                let rest = m.trim().strip_prefix(subject.trim()).unwrap_or("").trim().to_string();
                if !rest.is_empty() {
                    body.set_text(&rest);
                    body.set_visible(true);
                }
            }
        });

        *self.target.borrow_mut() = Some((None, c.oid.clone()));
        self.load_files();
    }

    /// Shows the difference between two commits.
    pub fn show_range(self: &Rc<Self>, from: &Commit, to: &Commit) {
        self.widget.set_visible_child_name("commit");
        while let Some(ch) = self.info.first_child() {
            self.info.remove(&ch);
        }
        let l = Self::value_label(&format!(
            "Showing changes from {} to {}",
            from.short(),
            to.short()
        ));
        l.add_css_class("heading");
        self.info.append(&l);
        self.info.append(&Self::value_label(&format!("{} — {}", from.short(), from.subject)));
        self.info.append(&Self::value_label(&format!("{} — {}", to.short(), to.subject)));
        *self.target.borrow_mut() = Some((Some(from.oid.clone()), to.oid.clone()));
        self.load_files();
    }

    /// Shows the staged and unstaged files, with the same staging controls
    /// as the File Status view.
    pub fn show_uncommitted(self: &Rc<Self>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let staging = self.staging.borrow().clone();
        let staging = staging.unwrap_or_else(|| {
            let st = StagingView::new(self.rv.clone(), 110, true);
            let pane = gtk::Paned::builder()
                .orientation(gtk::Orientation::Horizontal)
                .start_child(&st.lists)
                .end_child(&st.diff.widget)
                .shrink_start_child(false)
                .build();
            self.commit_pane
                .bind_property("position", &pane, "position")
                .bidirectional()
                .sync_create()
                .build();
            self.widget.add_named(&pane, Some("uncommitted"));
            *self.staging.borrow_mut() = Some(st.clone());
            st
        });
        // Drop any commit still loading into the commit pane.
        self.generation.set(self.generation.get() + 1);
        *self.target.borrow_mut() = None;
        self.widget.set_visible_child_name("uncommitted");
        staging.update(&rv.snapshot(), |e, _| !e.ignored);
        if !staging.has_selection() {
            staging.select_first();
        }
    }

    /// Refreshes the uncommitted changes after a status change, if shown.
    pub fn update_uncommitted(self: &Rc<Self>, snap: &Snapshot) {
        if self.widget.visible_child_name().as_deref() != Some("uncommitted") {
            return;
        }
        if let Some(st) = self.staging.borrow().as_ref() {
            st.update(snap, |e, _| !e.ignored);
        }
    }

    fn load_files(self: &Rc<Self>) {
        let Some((from, to)) = self.target.borrow().clone() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let git = self.git.clone();
        let filter = self.path_filter.clone();
        let this = self.clone();
        spawn(async move {
            let r = bg(move || match &from {
                None => diff::commit_files(&git, &to),
                Some(f) => diff::files_between(&git, f, &to),
            })
            .await;
            if this.generation.get() != generation {
                return;
            }
            match r {
                Ok(files) => {
                    let items: Vec<FileItem> = files
                        .into_iter()
                        .filter(|f| filter.as_ref().is_none_or(|p| &f.path == p || f.old_path.as_ref() == Some(p)))
                        .map(|f| FileItem {
                            path: f.path,
                            orig_path: f.old_path,
                            code: f.status,
                            untracked: false,
                            conflicted: false,
                        })
                        .collect();
                    let n = items.len();
                    this.files.set_files(items);
                    this.files.unselect_all();
                    if n > 0 {
                        this.files.select_first();
                    } else {
                        this.diff.show_message("No file changes");
                    }
                }
                Err(e) => this.diff.show_message(&e.to_string()),
            }
        });
    }

    fn load_diff(self: &Rc<Self>, files: Vec<FileItem>) {
        let Some((from, to)) = self.target.borrow().clone() else { return };
        if files.is_empty() {
            self.diff.show_message("No file selected");
            return;
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let git = self.git.clone();
        let opts = self.diff.options();
        let this = self.clone();
        let paths: Vec<String> = files.iter().take(50).map(|f| f.path.clone()).collect();
        let mut all_paths = paths.clone();
        for f in files.iter().take(50) {
            if let Some(o) = &f.orig_path {
                all_paths.push(o.clone());
            }
        }
        spawn(async move {
            let g2 = git.clone();
            let (f2, t2) = (from.clone(), to.clone());
            let r = bg(move || match &f2 {
                None => diff::commit(&g2, &t2, &all_paths, opts),
                Some(f) => diff::between(&g2, f, &t2, &all_paths, opts),
            })
            .await;
            if this.generation.get() != generation {
                return;
            }
            match r {
                Ok(d) => this.diff.show(
                    d,
                    DiffKind::ReadOnly,
                    Some(DiffContext {
                        git,
                        old_rev: Some(from.unwrap_or_else(|| format!("{to}^"))),
                        new_rev: Some(to),
                    }),
                ),
                Err(e) => this.diff.show_message(&e.to_string()),
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Commit list

struct GraphCell {
    area: gtk::DrawingArea,
    row: Rc<RefCell<Option<Rc<LogRow>>>>,
}

struct DescCell {
    badges: gtk::Box,
    label: gtk::Label,
}

type Batch = (Vec<LogRow>, bool);

pub struct HistoryView {
    pub widget: gtk::Box,
    rv: Weak<RepoView>,
    mode: HistoryMode,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    column_view: gtk::ColumnView,
    graph_col: gtk::ColumnViewColumn,
    scrolled: gtk::ScrolledWindow,
    pub details: Rc<CommitDetails>,
    scope: gtk::DropDown,
    order: gtk::DropDown,
    search_entry: gtk::SearchEntry,
    search_kind: gtk::DropDown,
    status_label: gtk::Label,
    req_tx: RefCell<Option<std::sync::mpsc::Sender<usize>>>,
    generation: Cell<u64>,
    loading: Cell<bool>,
    done: Cell<bool>,
    max_lanes: Cell<u16>,
    /// Horizontal scroll of the graph column, for graphs wider than it.
    graph_offset: Rc<Cell<f64>>,
    graph_areas: RefCell<Vec<glib::WeakRef<gtk::DrawingArea>>>,
    pending_select: RefCell<Option<String>>,
    selecting: Cell<bool>,
}

impl HistoryView {
    pub fn with_git(rv: Weak<RepoView>, git: Git, mode: HistoryMode) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Toolbar
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("pane-header");
        let scope = gtk::DropDown::from_strings(&["All Branches", "Local Branches", "Current Branch"]);
        let order = gtk::DropDown::from_strings(&["Date Order", "Ancestor Order"]);
        let search_kind = gtk::DropDown::from_strings(&["Commit Messages", "File Changes", "Authors", "Commit SHA"]);
        let search_entry = gtk::SearchEntry::builder().hexpand(true).build();
        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        match &mode {
            HistoryMode::History => {
                bar.append(&scope);
                bar.append(&order);
                let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                spacer.set_hexpand(true);
                bar.append(&spacer);
                bar.append(&status_label);
                search_entry.set_placeholder_text(Some("Jump to commit or ref"));
                search_entry.set_hexpand(false);
                search_entry.set_width_chars(22);
                bar.append(&search_entry);
            }
            HistoryMode::Search => {
                search_entry.set_placeholder_text(Some("Search"));
                bar.append(&search_kind);
                bar.append(&search_entry);
                bar.append(&status_label);
            }
            HistoryMode::FileLog(p) => {
                let l = gtk::Label::builder()
                    .label(format!("History of {p}"))
                    .xalign(0.0)
                    .hexpand(true)
                    .build();
                l.add_css_class("title");
                bar.append(&l);
                bar.append(&status_label);
            }
        }
        widget.append(&bar);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let column_view = gtk::ColumnView::builder()
            .model(&selection)
            .show_column_separators(true)
            .reorderable(false)
            .build();
        column_view.add_css_class("history-list");
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&column_view)
            .vexpand(true)
            .build();

        let path_filter = match &mode {
            HistoryMode::FileLog(p) => Some(p.clone()),
            _ => None,
        };
        let details = CommitDetails::new(rv.clone(), git, path_filter);
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&scrolled)
            .end_child(&details.widget)
            .position(340)
            .vexpand(true)
            .build();
        widget.append(&paned);

        let graph_col = gtk::ColumnViewColumn::builder()
            .title("Graph")
            .resizable(true)
            .fixed_width(60)
            .build();

        let this = Rc::new(Self {
            widget,
            rv,
            mode: mode.clone(),
            store,
            selection,
            column_view,
            graph_col,
            scrolled,
            details,
            scope,
            order,
            search_entry,
            search_kind,
            status_label,
            req_tx: RefCell::new(None),
            generation: Cell::new(0),
            loading: Cell::new(false),
            done: Cell::new(true),
            max_lanes: Cell::new(1),
            graph_offset: Rc::new(Cell::new(0.0)),
            graph_areas: RefCell::new(Vec::new()),
            pending_select: RefCell::new(None),
            selecting: Cell::new(false),
        });
        this.build_columns();
        this.connect_signals();
        this
    }

    fn attach_menu(self: &Rc<Self>, widget: &impl IsA<gtk::Widget>, item: &gtk::ListItem) {
        let gesture = gtk::GestureClick::builder().button(3).build();
        let w = Rc::downgrade(self);
        let item = item.downgrade();
        let wid = widget.clone().upcast::<gtk::Widget>();
        gesture.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            let (Some(t), Some(item)) = (w.upgrade(), item.upgrade()) else { return };
            let pos = item.position();
            if !t.selection.is_selected(pos) {
                t.selection.select_item(pos, true);
            }
            t.context_menu(&wid, x, y);
        });
        widget.add_controller(gesture);
    }

    fn build_columns(self: &Rc<Self>) {
        // Graph
        let f = gtk::SignalListItemFactory::new();
        let w = Rc::downgrade(self);
        f.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let area = gtk::DrawingArea::builder()
                .content_height(graph_cell::ROW_H)
                .build();
            let row: Rc<RefCell<Option<Rc<LogRow>>>> = Rc::new(RefCell::new(None));
            let r2 = row.clone();
            let t = w.upgrade();
            let offset = t.as_ref().map(|t| t.graph_offset.clone()).unwrap_or_default();
            area.set_draw_func(move |_, cr, _w, h| {
                if let Some(r) = r2.borrow().as_ref() {
                    cr.translate(-offset.get(), 0.0);
                    graph_cell::draw(
                        cr,
                        h as f64,
                        &r.graph,
                        &NodeStyle {
                            is_head: r.is_head,
                            uncommitted: r.uncommitted,
                        },
                    );
                }
            });
            item.set_child(Some(&area));
            if let Some(t) = t {
                t.attach_menu(&area, item);
                t.graph_areas.borrow_mut().push(area.downgrade());
            }
            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            let w = w.clone();
            scroll.connect_scroll(move |c, dx, dy| {
                if let (Some(t), Some((dx, only))) = (w.upgrade(), super::horizontal_scroll(c, dx, dy))
                    && t.scroll_graph(dx)
                    && only
                {
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            area.add_controller(scroll);
            unsafe { item.set_data("cell", Rc::new(GraphCell { area, row })) };
        });
        f.connect_bind(|_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let cell: Rc<GraphCell> = unsafe { item.data::<Rc<GraphCell>>("cell").unwrap().as_ref().clone() };
            *cell.row.borrow_mut() = item.item().and_then(|o| row_of(&o));
            cell.area.queue_draw();
        });
        self.graph_col.set_factory(Some(&f));
        if self.mode == HistoryMode::History {
            self.column_view.append_column(&self.graph_col);
            self.column_view.add_css_class("with-graph");
        }

        // Description with ref badges
        let f = gtk::SignalListItemFactory::new();
        let w = Rc::downgrade(self);
        f.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            hbox.set_height_request(graph_cell::ROW_H);
            let badges = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            badges.set_valign(gtk::Align::Center);
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .hexpand(true)
                .build();
            hbox.append(&badges);
            hbox.append(&label);
            item.set_child(Some(&hbox));
            if let Some(t) = w.upgrade() {
                t.attach_menu(&hbox, item);
            }
            unsafe { item.set_data("cell", Rc::new(DescCell { badges, label })) };
        });
        f.connect_bind(|_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let cell: Rc<DescCell> = unsafe { item.data::<Rc<DescCell>>("cell").unwrap().as_ref().clone() };
            while let Some(c) = cell.badges.first_child() {
                cell.badges.remove(&c);
            }
            let Some(r) = item.item().and_then(|o| row_of(&o)) else { return };
            if r.uncommitted {
                cell.label.set_text(&r.commit.subject);
                cell.label.add_css_class("uncommitted");
                return;
            }
            cell.label.remove_css_class("uncommitted");
            for rf in &r.refs {
                let l = gtk::Label::new(Some(&rf.name));
                l.add_css_class("ref-badge");
                l.add_css_class(match rf.kind {
                    RefKind::Local if rf.is_head => "head",
                    RefKind::Local => "local",
                    RefKind::Remote => "remote",
                    RefKind::Tag => "tag",
                });
                if rf.full == "HEAD" {
                    l.add_css_class("head");
                }
                cell.badges.append(&l);
            }
            cell.label.set_text(&r.commit.subject);
            cell.label.set_tooltip_text(Some(&r.commit.subject));
        });
        let desc = gtk::ColumnViewColumn::builder()
            .title("Description")
            .factory(&f)
            .expand(true)
            .resizable(true)
            .build();
        self.column_view.append_column(&desc);

        let text_col = |title: &str, width: i32, get: fn(&LogRow) -> String, mono: bool| {
            let f = gtk::SignalListItemFactory::new();
            let w = Rc::downgrade(self);
            f.connect_setup(move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let l = gtk::Label::builder()
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .height_request(graph_cell::ROW_H)
                    .build();
                if mono {
                    l.add_css_class("mono");
                }
                l.add_css_class("dim-label-2");
                item.set_child(Some(&l));
                if let Some(t) = w.upgrade() {
                    t.attach_menu(&l, item);
                }
            });
            f.connect_bind(move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let Some(l) = item.child().and_downcast::<gtk::Label>() else { return };
                let Some(r) = item.item().and_then(|o| row_of(&o)) else { return };
                l.set_text(&if r.uncommitted { String::new() } else { get(&r) });
            });
            gtk::ColumnViewColumn::builder()
                .title(title)
                .factory(&f)
                .resizable(true)
                .fixed_width(width)
                .build()
        };
        self.column_view.append_column(&text_col("Date", 170, |r| format_time(r.commit.author_time), false));
        self.column_view.append_column(&text_col("Author", 150, |r| r.commit.author_name.clone(), false));
        self.column_view.append_column(&text_col("Commit", 80, |r| r.commit.short().to_string(), true));
    }

    /// Scrolls the graph sideways by `dx`. Returns false when it all fits.
    fn scroll_graph(&self, dx: f64) -> bool {
        let max = (graph_cell::width_for(self.max_lanes.get()) - self.graph_col.fixed_width()).max(0) as f64;
        let v = (self.graph_offset.get() + dx).clamp(0.0, max);
        if max == 0.0 && v == self.graph_offset.get() {
            return false;
        }
        self.graph_offset.set(v);
        self.graph_areas.borrow_mut().retain(|a| match a.upgrade() {
            Some(a) => {
                a.queue_draw();
                true
            }
            None => false,
        });
        true
    }

    fn connect_signals(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(t) = w.upgrade()
                && !t.selecting.get() {
                    t.on_selection();
                }
        });
        let w = Rc::downgrade(self);
        self.scrolled.vadjustment().connect_value_changed(move |adj| {
            if let Some(t) = w.upgrade()
                && adj.value() + adj.page_size() * 3.0 >= adj.upper() {
                    t.load_more(0);
                }
        });
        let w = Rc::downgrade(self);
        self.column_view.connect_activate(move |_, pos| {
            let Some(t) = w.upgrade() else { return };
            let Some(r) = t.store.item(pos).and_then(|o| row_of(&o)) else { return };
            let Some(rv) = t.rv.upgrade() else { return };
            if r.uncommitted {
                rv.show_view(View::Status);
            } else if t.mode != HistoryMode::History {
                rv.jump_to_commit(&r.commit.oid);
            }
        });
        for dd in [&self.scope, &self.order] {
            let w = Rc::downgrade(self);
            dd.connect_selected_notify(move |_| {
                if let Some(t) = w.upgrade() {
                    t.reload();
                }
            });
        }
        let w = Rc::downgrade(self);
        self.search_entry.connect_activate(move |e| {
            let Some(t) = w.upgrade() else { return };
            match t.mode {
                HistoryMode::History => {
                    let q = e.text().trim().to_string();
                    if q.is_empty() {
                        return;
                    }
                    let Some(rv) = t.rv.upgrade() else { return };
                    match rv.git.run(&["rev-parse", "--verify", "-q", &format!("{q}^{{commit}}")]) {
                        Ok(oid) => t.select_commit(oid.trim()),
                        Err(_) => rv.toast(&format!("No commit or ref named “{q}”")),
                    }
                }
                _ => t.reload(),
            }
        });
        if self.mode == HistoryMode::Search {
            let w = Rc::downgrade(self);
            self.search_kind.connect_selected_notify(move |_| {
                if let Some(t) = w.upgrade() {
                    t.reload();
                }
            });
        }
    }

    pub fn debug_search(self: &Rc<Self>, text: &str) {
        self.search_entry.set_text(text);
        self.reload();
    }

    pub fn debug_select(&self, index: u32) {
        self.selection.select_item(index, true);
    }

    pub fn focus_search(&self) {
        self.search_entry.grab_focus();
    }

    fn selected_rows(&self) -> Vec<Rc<LogRow>> {
        let mut out = Vec::new();
        let bits = self.selection.selection();
        let n = bits.size();
        for i in 0..n.min(200) {
            let pos = bits.nth(i as u32);
            if let Some(r) = self.store.item(pos).and_then(|o| row_of(&o)) {
                out.push(r);
            }
        }
        out
    }

    fn on_selection(self: &Rc<Self>) {
        let rows = self.selected_rows();
        match rows.len() {
            0 => self.details.clear(),
            1 => {
                let r = &rows[0];
                if r.uncommitted {
                    self.details.show_uncommitted();
                } else {
                    self.details.show_commit(&r.commit, &r.refs);
                }
            }
            _ => {
                let a = rows.iter().find(|r| !r.uncommitted);
                let b = rows.iter().rev().find(|r| !r.uncommitted);
                if let (Some(newer), Some(older)) = (a, b)
                    && !Rc::ptr_eq(newer, older) {
                        self.details.show_range(&older.commit, &newer.commit);
                    }
            }
        }
    }

    fn query(&self, rv: &RepoView) -> Option<LogQuery> {
        let snap = rv.snapshot();
        let has_head = snap.refs.head_oid.is_some();
        let mut q = LogQuery {
            has_head,
            ..Default::default()
        };
        match &self.mode {
            HistoryMode::History => {
                q.scope = match self.scope.selected() {
                    1 => LogScope::Local,
                    2 => LogScope::Current,
                    _ => LogScope::All,
                };
                q.order = if self.order.selected() == 1 {
                    LogOrder::Topo
                } else {
                    LogOrder::Date
                };
            }
            HistoryMode::Search => {
                let text = self.search_entry.text().trim().to_string();
                if text.is_empty() {
                    return None;
                }
                match self.search_kind.selected() {
                    0 => q.extra = vec![format!("--grep={text}"), "-i".into(), "--all-match".into()],
                    1 => q.extra = vec![format!("-S{text}")],
                    2 => q.extra = vec![format!("--author={text}"), "-i".into()],
                    _ => {
                        q.rev = Some(text);
                        q.extra = vec!["--no-walk".into()];
                    }
                }
            }
            HistoryMode::FileLog(p) => {
                q.scope = LogScope::Current;
                q.paths = vec![p.clone()];
                q.follow = true;
            }
        }
        if !has_head && snap.refs.refs.is_empty() {
            return None;
        }
        Some(q)
    }

    /// Restarts loading the commit list.
    pub fn reload(self: &Rc<Self>) {
        let Some(rv) = self.rv.upgrade() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        *self.req_tx.borrow_mut() = None;
        let prev_selected: Vec<String> = self.selected_rows().iter().map(|r| r.commit.oid.clone()).collect();
        let scroll = self.scrolled.vadjustment().value();

        let Some(query) = self.query(&rv) else {
            self.store.remove_all();
            self.details.clear();
            self.status_label.set_text("");
            self.done.set(true);
            return;
        };
        let snap = rv.snapshot();
        let refs_map = snap.refs.by_oid();
        let head = snap.refs.head_oid.clone();
        let detached = snap.refs.head_branch.is_none();
        let uncommitted = self.mode == HistoryMode::History && snap.has_changes() && head.is_some();
        let git = rv.git.clone();
        let page = config::with(|s| s.log_page_size).max(200) as usize;

        let (req_tx, req_rx) = std::sync::mpsc::channel::<usize>();
        let (tx, rx) = async_channel::unbounded::<Batch>();
        std::thread::spawn(move || {
            let reader = match LogReader::spawn(&git, &query) {
                Ok(r) => r,
                Err(_) => {
                    let _ = tx.send_blocking((Vec::new(), true));
                    return;
                }
            };
            let mut reader = reader;
            let mut graph = GraphBuilder::new();
            let mut first = true;
            while let Ok(n) = req_rx.recv() {
                let mut batch = Vec::with_capacity(n + 1);
                if first && uncommitted {
                    let h = head.clone().unwrap_or_default();
                    let g = graph.push(UNCOMMITTED, std::slice::from_ref(&h));
                    batch.push(LogRow {
                        commit: Commit {
                            oid: UNCOMMITTED.into(),
                            parents: vec![h],
                            subject: "Uncommitted changes".into(),
                            ..Default::default()
                        },
                        graph: g,
                        refs: Vec::new(),
                        is_head: false,
                        uncommitted: true,
                    });
                }
                first = false;
                let mut count = 0;
                for c in reader.by_ref() {
                    let g = graph.push(&c.oid, &c.parents);
                    let mut refs = refs_map.get(&c.oid).cloned().unwrap_or_default();
                    // Current branch first, then locals, remotes, tags.
                    refs.sort_by_key(|r| (!r.is_head, r.kind as u8));
                    let is_head = head.as_deref() == Some(c.oid.as_str());
                    if is_head && detached {
                        refs.insert(
                            0,
                            RefInfo {
                                full: "HEAD".into(),
                                name: "HEAD".into(),
                                kind: RefKind::Local,
                                oid: c.oid.clone(),
                                upstream: None,
                                ahead: 0,
                                behind: 0,
                                upstream_gone: false,
                                is_head: true,
                                subject: String::new(),
                            },
                        );
                    }
                    batch.push(LogRow {
                        commit: c,
                        graph: g,
                        refs,
                        is_head,
                        uncommitted: false,
                    });
                    count += 1;
                    if count >= n {
                        break;
                    }
                }
                let finished = count < n;
                if tx.send_blocking((batch, finished)).is_err() || finished {
                    break;
                }
            }
        });

        self.loading.set(true);
        self.done.set(false);
        self.max_lanes.set(1);
        self.graph_offset.set(0.0);
        let _ = req_tx.send(page);
        *self.req_tx.borrow_mut() = Some(req_tx);
        self.status_label.set_text("Loading…");

        let this = self.clone();
        spawn(async move {
            let mut first = true;
            while let Ok((batch, finished)) = rx.recv().await {
                if this.generation.get() != generation {
                    return;
                }
                let mut lanes = this.max_lanes.get();
                let objs: Vec<glib::BoxedAnyObject> = batch
                    .into_iter()
                    .map(|r| {
                        lanes = lanes.max(r.graph.width);
                        glib::BoxedAnyObject::new(Rc::new(r))
                    })
                    .collect();
                this.max_lanes.set(lanes);
                this.graph_col
                    .set_fixed_width(graph_cell::width_for(lanes.min(24)));
                this.selecting.set(true);
                if first {
                    this.store.splice(0, this.store.n_items(), &objs);
                } else {
                    this.store.extend_from_slice(&objs);
                }
                this.selecting.set(false);
                this.loading.set(false);
                if finished {
                    this.done.set(true);
                }
                let n = this.store.n_items();
                this.status_label.set_text(&format!(
                    "{}{} commits",
                    n,
                    if this.done.get() { "" } else { "+" }
                ));
                if first {
                    first = false;
                    // Restore previous selection / scroll position.
                    if !prev_selected.is_empty() && this.pending_select.borrow().is_none() {
                        let found = this.find(&prev_selected[0]);
                        match found {
                            Some(pos) => {
                                this.selecting.set(true);
                                this.selection.select_item(pos, true);
                                this.selecting.set(false);
                                this.on_selection();
                                let adj = this.scrolled.vadjustment();
                                glib::idle_add_local_once(move || adj.set_value(scroll));
                            }
                            None => this.details.clear(),
                        }
                    } else if prev_selected.is_empty() && this.pending_select.borrow().is_none() {
                        this.details.clear();
                    }
                }
                let pending = this.pending_select.borrow().clone();
                if let Some(oid) = pending {
                    if let Some(pos) = this.find(&oid) {
                        *this.pending_select.borrow_mut() = None;
                        this.scroll_select(pos);
                    } else if this.done.get() {
                        *this.pending_select.borrow_mut() = None;
                        if let Some(rv) = this.rv.upgrade() {
                            rv.toast("That commit is not shown with the current branch filter");
                        }
                    } else {
                        this.load_more(5000);
                    }
                }
                if finished {
                    break;
                }
            }
        });
    }

    fn load_more(&self, n: usize) {
        if self.done.get() || self.loading.get() {
            return;
        }
        let page = if n > 0 {
            n
        } else {
            config::with(|s| s.log_page_size).max(200) as usize
        };
        if let Some(tx) = self.req_tx.borrow().as_ref()
            && tx.send(page).is_ok() {
                self.loading.set(true);
            }
    }

    fn find(&self, oid: &str) -> Option<u32> {
        (0..self.store.n_items()).find(|&i| {
            self.store
                .item(i)
                .and_then(|o| row_of(&o))
                .is_some_and(|r| r.commit.oid == oid || (oid.len() < 40 && r.commit.oid.starts_with(oid)))
        })
    }

    fn scroll_select(&self, pos: u32) {
        self.column_view.scroll_to(
            pos,
            None::<&gtk::ColumnViewColumn>,
            gtk::ListScrollFlags::SELECT | gtk::ListScrollFlags::FOCUS,
            None,
        );
    }

    /// Selects `oid`, loading more history if needed.
    pub fn select_commit(self: &Rc<Self>, oid: &str) {
        if let Some(pos) = self.find(oid) {
            self.scroll_select(pos);
            return;
        }
        *self.pending_select.borrow_mut() = Some(oid.to_string());
        if self.done.get() {
            // Not loaded and nothing more to load: maybe the filter hides it.
            *self.pending_select.borrow_mut() = None;
            if let Some(rv) = self.rv.upgrade() {
                rv.toast("That commit is not shown with the current branch filter");
            }
        } else {
            self.load_more(5000);
        }
    }

    fn context_menu(&self, widget: &gtk::Widget, x: f64, y: f64) {
        let rows: Vec<Rc<LogRow>> = self.selected_rows().into_iter().filter(|r| !r.uncommitted).collect();
        if rows.is_empty() {
            return;
        }
        let Some(rv) = self.rv.upgrade() else { return };
        let current = rv.snapshot().current_branch().unwrap_or("HEAD").to_string();
        let menu = gio::Menu::new();
        if rows.len() == 1 {
            let c = &rows[0].commit;
            let oid = c.oid.as_str();
            let s1 = gio::Menu::new();
            menu_item_target(&s1, "Checkout…", "repo.checkout-commit", oid);
            menu_item_target(&s1, &format!("Merge into {current}…"), "repo.merge-commit", oid);
            menu_item_target(&s1, &format!("Rebase {current} onto this…"), "repo.rebase-onto", oid);
            menu.append_section(None, &s1);
            let s2 = gio::Menu::new();
            menu_item_target(&s2, "Tag…", "repo.tag", oid);
            menu_item_target(&s2, "Branch…", "repo.branch", oid);
            menu_item_target(&s2, "Archive…", "repo.archive", oid);
            menu_item_target(&s2, "Create Patch…", "repo.patch", oid);
            menu.append_section(None, &s2);
            let s3 = gio::Menu::new();
            menu_item_target(&s3, &format!("Reset {current} to this commit…"), "repo.reset-to", oid);
            menu_item_target(&s3, "Reverse Commit…", "repo.revert", oid);
            menu_item_target(&s3, "Cherry Pick", "repo.cherry-pick", oid);
            menu_item_target(&s3, &format!("Rebase children of {} interactively…", c.short()), "repo.rebase-interactive", oid);
            menu.append_section(None, &s3);
            let s4 = gio::Menu::new();
            menu_item_target(&s4, "Copy SHA to Clipboard", "repo.copy-text", oid);
            menu_item_target(&s4, "Copy Commit Message", "repo.copy-message", oid);
            menu.append_section(None, &s4);
            let actions = config::with(|s| s.custom_actions.clone());
            if !actions.is_empty() {
                let s5 = gio::Menu::new();
                for (i, a) in actions.iter().enumerate() {
                    menu_item_target(&s5, &a.name, "repo.custom-action", &format!("{i}|{oid}|"));
                }
                menu.append_submenu(Some("Custom Actions"), &s5);
            }
        } else {
            // Oldest first for cherry-picking.
            let oids: Vec<String> = rows.iter().rev().map(|r| r.commit.oid.clone()).collect();
            let s1 = gio::Menu::new();
            menu_item_target(&s1, &format!("Cherry Pick {} Commits", oids.len()), "repo.cherry-pick", &oids.join(" "));
            menu_item_target(&s1, "Copy SHAs to Clipboard", "repo.copy-text", &oids.join("\n"));
            menu.append_section(None, &s1);
        }
        popup_menu(widget, x, y, &menu);
    }
}

/// Stand-alone window showing the log of a single file.
pub fn file_log_window(rv: &Rc<RepoView>, path: &str) {
    let win = adw::Window::builder()
        .title(format!("Log — {path}"))
        .default_width(1100)
        .default_height(760)
        .build();
    if let Some(root) = rv.widget.root().and_downcast::<gtk::Window>() {
        win.set_transient_for(Some(&root));
    }
    let view = HistoryView::with_git(rv.weak(), rv.git.clone(), HistoryMode::FileLog(path.to_string()));
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    let toast = adw::ToastOverlay::new();
    toast.set_child(Some(&view.widget));
    tv.set_content(Some(&toast));
    win.set_content(Some(&tv));
    // Actions (context menus) resolve against the repo view's group.
    win.insert_action_group("repo", Some(&rv.actions));
    view.reload();
    win.present();
    // Keep the view alive as long as the window.
    let holder = RefCell::new(Some(view));
    win.connect_close_request(move |_| {
        holder.borrow_mut().take();
        glib::Propagation::Proceed
    });
}

/// Window showing a single commit (used for stashes).
pub fn commit_window(rv: &Rc<RepoView>, title: &str, commit: Commit) {
    let win = adw::Window::builder()
        .title(title)
        .default_width(1000)
        .default_height(700)
        .build();
    if let Some(root) = rv.widget.root().and_downcast::<gtk::Window>() {
        win.set_transient_for(Some(&root));
    }
    let details = CommitDetails::new(rv.weak(), rv.git.clone(), None);
    details.show_commit(&commit, &[]);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&details.widget));
    win.set_content(Some(&tv));
    win.insert_action_group("repo", Some(&rv.actions));
    win.present();
    let holder = RefCell::new(Some(details));
    win.connect_close_request(move |_| {
        holder.borrow_mut().take();
        glib::Propagation::Proceed
    });
}

