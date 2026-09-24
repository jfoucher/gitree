//! List of changed files (flat or tree), with optional stage checkboxes.

use super::{status_badge, status_tooltip};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    pub path: String,
    pub orig_path: Option<String>,
    pub code: char,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, PartialEq)]
struct FNode {
    name: String,
    /// File path, or directory path for folders.
    full: String,
    file: Option<FileItem>,
    /// Updated in place when a folder row is kept across a refresh.
    children: RefCell<Vec<Rc<FNode>>>,
}

impl FNode {
    fn files(&self, out: &mut Vec<FileItem>) {
        if let Some(f) = &self.file {
            out.push(f.clone());
        }
        for c in self.children.borrow().iter() {
            c.files(out);
        }
    }
}

fn build_tree(items: &[FileItem]) -> Vec<Rc<FNode>> {
    #[derive(Default)]
    struct Dir {
        dirs: std::collections::BTreeMap<String, Dir>,
        files: Vec<FileItem>,
    }
    let mut root = Dir::default();
    for it in items {
        let parts: Vec<&str> = it.path.split('/').collect();
        let mut d = &mut root;
        for p in &parts[..parts.len() - 1] {
            d = d.dirs.entry(p.to_string()).or_default();
        }
        d.files.push(it.clone());
    }
    fn convert(prefix: &str, d: Dir) -> Vec<Rc<FNode>> {
        let mut out = Vec::new();
        for (name, sub) in d.dirs {
            // Collapse single-child directory chains: "src/ui".
            let mut name = name;
            let mut sub = sub;
            while sub.files.is_empty() && sub.dirs.len() == 1 {
                let (n, s) = sub.dirs.into_iter().next().unwrap();
                name = format!("{name}/{n}");
                sub = s;
            }
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let children = convert(&full, sub);
            out.push(Rc::new(FNode {
                name,
                full,
                file: None,
                children: RefCell::new(children),
            }));
        }
        let mut files = d.files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        for f in files {
            out.push(Rc::new(FNode {
                name: f.path.rsplit('/').next().unwrap_or(&f.path).to_string(),
                full: f.path.clone(),
                file: Some(f),
                children: RefCell::new(Vec::new()),
            }));
        }
        out
    }
    convert("", root)
}

fn node_of(obj: &glib::Object) -> Option<Rc<FNode>> {
    let row = obj.downcast_ref::<gtk::TreeListRow>()?;
    let item = row.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    let n = item.borrow::<Rc<FNode>>().clone();
    Some(n)
}

struct RowCell {
    expander: gtk::TreeExpander,
    check: gtk::CheckButton,
    badge: gtk::Label,
    name: gtk::Label,
    dir: gtk::Label,
    node: RefCell<Option<Rc<FNode>>>,
    binding: Cell<bool>,
}

type FilesCb = RefCell<Option<Box<dyn Fn(Vec<FileItem>)>>>;
type ContextCb = RefCell<Option<Box<dyn Fn(&gtk::Widget, f64, f64, Vec<FileItem>)>>>;

pub struct FileList {
    pub widget: gtk::ScrolledWindow,
    pub list: gtk::ListView,
    root: gio::ListStore,
    model: gtk::TreeListModel,
    pub selection: gtk::MultiSelection,
    tree_mode: Cell<bool>,
    items: RefCell<Vec<FileItem>>,
    suppress: Cell<bool>,
    pub on_toggle: FilesCb,
    pub on_selection: FilesCb,
    pub on_activate: FilesCb,
    pub on_context: ContextCb,
}

impl FileList {
    pub fn new(checked: Option<bool>) -> Rc<Self> {
        let root = gio::ListStore::new::<glib::BoxedAnyObject>();
        let model = gtk::TreeListModel::new(root.clone(), false, true, |obj| {
            let item = obj.downcast_ref::<glib::BoxedAnyObject>()?;
            let node = item.borrow::<Rc<FNode>>();
            if node.file.is_some() {
                return None;
            }
            let store = gio::ListStore::new::<glib::BoxedAnyObject>();
            for c in node.children.borrow().iter() {
                store.append(&glib::BoxedAnyObject::new(c.clone()));
            }
            Some(store.upcast())
        });
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .build();
        list.add_css_class("navigation-sidebar");
        let widget = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let this = Rc::new(Self {
            widget,
            list,
            root,
            model,
            selection,
            tree_mode: Cell::new(false),
            items: RefCell::new(Vec::new()),
            suppress: Cell::new(false),
            on_toggle: RefCell::new(None),
            on_selection: RefCell::new(None),
            on_activate: RefCell::new(None),
            on_context: RefCell::new(None),
        });

        let weak = Rc::downgrade(&this);
        factory.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let check = gtk::CheckButton::new();
            check.set_visible(checked.is_some());
            let badge = gtk::Label::new(None);
            badge.add_css_class("status-icon");
            let name = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let dir = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Start)
                .build();
            dir.add_css_class("dim-label");
            dir.add_css_class("caption");
            hbox.append(&check);
            hbox.append(&badge);
            hbox.append(&name);
            hbox.append(&dir);
            let expander = gtk::TreeExpander::builder()
                .child(&hbox)
                .indent_for_icon(false)
                .build();
            item.set_child(Some(&expander));
            let cell = Rc::new(RowCell {
                expander: expander.clone(),
                check: check.clone(),
                badge,
                name,
                dir,
                node: RefCell::new(None),
                binding: Cell::new(false),
            });

            let c2 = cell.clone();
            let w2 = weak.clone();
            check.connect_toggled(move |_| {
                if c2.binding.get() {
                    return;
                }
                let node = c2.node.borrow().clone();
                if let (Some(n), Some(fl)) = (node, w2.upgrade()) {
                    let mut files = Vec::new();
                    n.files(&mut files);
                    if let Some(cb) = fl.on_toggle.borrow().as_ref() {
                        cb(files);
                    }
                }
            });

            let gesture = gtk::GestureClick::builder().button(3).build();
            let c3 = cell.clone();
            let w3 = weak.clone();
            let e3 = expander.clone();
            gesture.connect_pressed(move |g, _, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                let (Some(fl), Some(n)) = (w3.upgrade(), c3.node.borrow().clone()) else { return };
                fl.context_at(&e3, x, y, &n);
            });
            expander.add_controller(gesture);
            unsafe {
                item.set_data("cell", cell);
            }
        });
        factory.connect_bind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let cell: Rc<RowCell> = unsafe { item.data::<Rc<RowCell>>("cell").unwrap().as_ref().clone() };
            let row = item.item().and_downcast::<gtk::TreeListRow>();
            cell.expander.set_list_row(row.as_ref());
            let Some(node) = item.item().and_then(|o| node_of(&o)) else { return };
            cell.binding.set(true);
            if let Some(c) = checked {
                cell.check.set_active(c);
            }
            for c in ["A", "M", "D", "R", "C", "U", "Q", "I"] {
                cell.badge.remove_css_class(&format!("status-{c}"));
            }
            match &node.file {
                Some(f) => {
                    let (txt, cls) = status_badge(f.code);
                    cell.badge.set_text(&txt);
                    cell.badge.add_css_class(&format!("status-{cls}"));
                    cell.badge.set_visible(true);
                    cell.badge.set_tooltip_text(Some(status_tooltip(f.code)));
                    cell.name.set_text(&node.name);
                    let tree = row.as_ref().is_some_and(|r| r.depth() > 0)
                        || !f.path.contains('/');
                    let parent = f.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    let mut d = if tree { String::new() } else { parent.to_string() };
                    if let Some(o) = &f.orig_path {
                        d = format!("{d}  ← {o}");
                    }
                    cell.dir.set_text(&d);
                    cell.expander.set_tooltip_text(Some(&f.path));
                }
                None => {
                    cell.badge.set_visible(false);
                    cell.name.set_text(&node.name);
                    cell.dir.set_text("");
                    cell.expander.set_tooltip_text(Some(&node.full));
                }
            }
            *cell.node.borrow_mut() = Some(node);
            cell.binding.set(false);
        });

        let weak = Rc::downgrade(&this);
        this.selection.connect_selection_changed(move |_, _, _| {
            let Some(fl) = weak.upgrade() else { return };
            if fl.suppress.get() {
                return;
            }
            let files = fl.selected_files();
            if let Some(cb) = fl.on_selection.borrow().as_ref() {
                cb(files);
            }
        });
        let weak = Rc::downgrade(&this);
        this.list.connect_activate(move |_, pos| {
            let Some(fl) = weak.upgrade() else { return };
            let Some(n) = fl.model.item(pos).and_then(|o| node_of(&o)) else { return };
            let mut files = Vec::new();
            n.files(&mut files);
            if let Some(cb) = fl.on_activate.borrow().as_ref() {
                cb(files);
            }
        });
        this
    }

    fn context_at(&self, w: &gtk::TreeExpander, x: f64, y: f64, node: &Rc<FNode>) {
        // Select the clicked row if it is not part of the selection.
        let pos = (0..self.model.n_items()).find(|&i| {
            self.model
                .item(i)
                .and_then(|o| node_of(&o))
                .is_some_and(|n| Rc::ptr_eq(&n, node))
        });
        if let Some(p) = pos
            && !self.selection.is_selected(p) {
                self.selection.select_item(p, true);
            }
        let files = self.selected_files();
        if let Some(cb) = self.on_context.borrow().as_ref() {
            cb(w.upcast_ref(), x, y, files);
        }
    }

    pub fn selected_files(&self) -> Vec<FileItem> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for i in 0..self.model.n_items() {
            if self.selection.is_selected(i)
                && let Some(n) = self.model.item(i).and_then(|o| node_of(&o)) {
                    let mut v = Vec::new();
                    n.files(&mut v);
                    for f in v {
                        if seen.insert(f.path.clone()) {
                            out.push(f);
                        }
                    }
                }
        }
        out
    }

    pub fn all_files(&self) -> Vec<FileItem> {
        self.items.borrow().clone()
    }

    pub fn set_tree_mode(&self, tree: bool) {
        if self.tree_mode.replace(tree) != tree {
            let items = self.items.borrow().clone();
            self.rebuild(&items, true);
        }
    }

    /// Replaces the file list, keeping the selection where possible.
    /// Returns true if the list content changed.
    pub fn set_files(&self, items: Vec<FileItem>) -> bool {
        if *self.items.borrow() == items {
            return false;
        }
        self.rebuild(&items, false);
        *self.items.borrow_mut() = items;
        true
    }

    fn rebuild(&self, items: &[FileItem], notify: bool) {
        let prev: HashSet<String> = self.selected_files().into_iter().map(|f| f.path).collect();
        self.suppress.set(true);
        let nodes: Vec<Rc<FNode>> = if self.tree_mode.get() {
            build_tree(items)
        } else {
            let mut v: Vec<Rc<FNode>> = items
                .iter()
                .map(|f| {
                    Rc::new(FNode {
                        name: f.path.rsplit('/').next().unwrap_or(&f.path).to_string(),
                        full: f.path.clone(),
                        file: Some(f.clone()),
                        children: RefCell::new(Vec::new()),
                    })
                })
                .collect();
            v.sort_by(|a, b| a.full.cmp(&b.full));
            v
        };
        self.sync(&self.root, None, &nodes);
        self.selection.unselect_all();
        for i in 0..self.model.n_items() {
            if let Some(n) = self.model.item(i).and_then(|o| node_of(&o))
                && n.file.as_ref().is_some_and(|f| prev.contains(&f.path)) {
                    self.selection.select_item(i, false);
                }
        }
        self.suppress.set(false);
        let now: HashSet<String> = self.selected_files().into_iter().map(|f| f.path).collect();
        if (notify || now != prev)
            && let Some(cb) = self.on_selection.borrow().as_ref() {
                cb(self.selected_files());
            }
    }

    /// Updates `store` (the children of `parent`, or the top level) to
    /// `new`, replacing only the rows that changed and keeping folder rows
    /// in place, so the list keeps its scroll position and expanded folders
    /// (e.g. when a file moves to the other list).
    fn sync(&self, store: &gio::ListStore, parent: Option<&gtk::TreeListRow>, new: &[Rc<FNode>]) {
        let old: Vec<Rc<FNode>> = (0..store.n_items())
            .filter_map(|i| store.item(i))
            .filter_map(|o| o.downcast::<glib::BoxedAnyObject>().ok())
            .map(|b| b.borrow::<Rc<FNode>>().clone())
            .collect();
        let keep = |a: &&Rc<FNode>, b: &&Rc<FNode>| {
            a == b || (a.file.is_none() && b.file.is_none() && a.full == b.full && a.name == b.name)
        };
        let prefix = old.iter().zip(new).take_while(|(a, b)| keep(a, b)).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| keep(a, b))
            .count();
        let removed = old.len() - prefix - suffix;
        let added = &new[prefix..new.len() - suffix];
        if removed > 0 {
            self.keep_focus_outside(parent, prefix as u32, (prefix + removed) as u32);
        }
        let objs: Vec<glib::BoxedAnyObject> = added.iter().cloned().map(glib::BoxedAnyObject::new).collect();
        store.splice(prefix as u32, removed as u32, &objs);

        // Kept folders whose content changed: update their children in place.
        let kept = (0..prefix).chain(old.len() - suffix..old.len());
        for i in kept {
            let (a, b) = (&old[i], &new[if i < prefix { i } else { i + new.len() - old.len() }]);
            if a == b {
                continue;
            }
            let pos = if i < prefix { i } else { i + added.len() - removed } as u32;
            let row = match parent {
                Some(p) => p.child_row(pos),
                None => self.model.child_row(pos),
            };
            let children = row.as_ref().and_then(|r| r.children()).and_downcast::<gio::ListStore>();
            match children {
                Some(child_store) => {
                    self.sync(&child_store, row.as_ref(), &b.children.borrow());
                    *a.children.borrow_mut() = b.children.borrow().clone();
                }
                // Collapsed: the children are read when it is expanded.
                None => *a.children.borrow_mut() = b.children.borrow().clone(),
            }
        }
    }

    /// Moves keyboard focus off the rows of items `start..end` of `parent`
    /// (or of the top level) before they are removed. Otherwise the list
    /// view moves focus to its first row and scrolls to the top (e.g. after
    /// clicking a checkbox, which focuses its row and then moves the file to
    /// the other list).
    fn keep_focus_outside(&self, parent: Option<&gtk::TreeListRow>, start: u32, end: u32) {
        let Some(focused) = self.focused_row() else { return };
        let row_at = |i: u32| match parent {
            Some(p) => p.child_row(i),
            None => self.model.child_row(i),
        };
        let (Some(first), Some(last)) = (row_at(start), row_at(end - 1)) else { return };
        // The removed rows span from `first` to the end of `last`'s subtree.
        let n = self.model.n_items();
        let mut after = last.position() + 1;
        while after < n && self.model.row(after).is_some_and(|r| r.depth() > last.depth()) {
            after += 1;
        }
        let first = first.position();
        if !(first..after).contains(&focused) {
            return;
        }
        let target = if after < n {
            after
        } else if first > 0 {
            first - 1
        } else {
            return;
        };
        self.list.scroll_to(target, gtk::ListScrollFlags::FOCUS, None);
    }

    /// Position of the row that holds keyboard focus, if it is in this list.
    fn focused_row(&self) -> Option<u32> {
        let mut w = self.list.root()?.focus()?;
        if !w.is_ancestor(&self.list) {
            return None;
        }
        loop {
            let expander = w
                .downcast_ref::<gtk::TreeExpander>()
                .cloned()
                .or_else(|| w.first_child().and_downcast::<gtk::TreeExpander>());
            if let Some(e) = expander {
                return e.list_row().map(|r| r.position());
            }
            w = w.parent()?;
            if w == *self.list.upcast_ref::<gtk::Widget>() {
                return None;
            }
        }
    }

    pub fn unselect_all(&self) {
        self.suppress.set(true);
        self.selection.unselect_all();
        self.suppress.set(false);
    }

    pub fn select_first(&self) {
        if self.model.n_items() > 0 {
            // Select the first file row (skip folders).
            for i in 0..self.model.n_items() {
                if self
                    .model
                    .item(i)
                    .and_then(|o| node_of(&o))
                    .is_some_and(|n| n.file.is_some())
                {
                    self.selection.select_item(i, true);
                    return;
                }
            }
        }
    }
}
