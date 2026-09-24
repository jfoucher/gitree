//! Repository sidebar: WORKSPACE, BRANCHES, TAGS, REMOTES, STASHES,
//! SUBMODULES and SUBTREES, as a collapsible tree.

use super::repo_view::{RepoView, Snapshot, View};
use super::{menu_item_target, popup_menu};
use crate::git::refs::RefInfo;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

#[derive(Debug, Clone)]
pub enum Kind {
    Section,
    View(View),
    Folder,
    Branch(RefInfo),
    Remote(String),
    RemoteBranch(RefInfo),
    Tag(RefInfo),
    Stash(String),
    Submodule(String),
    Subtree(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub key: String,
    pub label: String,
    pub icon: &'static str,
    pub kind: Kind,
    pub detail: String,
    pub bold: bool,
    pub children: Vec<Rc<Node>>,
}

impl Node {
    fn new(key: impl Into<String>, label: impl Into<String>, icon: &'static str, kind: Kind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            icon,
            kind,
            detail: String::new(),
            bold: false,
            children: Vec::new(),
        }
    }
}

/// Builds a folder hierarchy from slash-separated names ("feature/login").
fn insert_path(parent: &mut Node, key_prefix: &str, parts: &[&str], leaf: Node) {
    if parts.len() <= 1 {
        parent.children.push(Rc::new(leaf));
        return;
    }
    let dir = parts[0];
    let key = format!("{key_prefix}/{dir}");
    let idx = parent
        .children
        .iter()
        .position(|c| matches!(c.kind, Kind::Folder) && c.label == dir);
    let idx = match idx {
        Some(i) => i,
        None => {
            parent
                .children
                .push(Rc::new(Node::new(key.clone(), dir, "folder-symbolic", Kind::Folder)));
            parent.children.len() - 1
        }
    };
    let child = Rc::make_mut(&mut parent.children[idx]);
    insert_path(child, &key, &parts[1..], leaf);
}

fn build_tree(s: &Snapshot) -> Vec<Rc<Node>> {
    let mut roots = Vec::new();

    let mut ws = Node::new("ws", "WORKSPACE", "", Kind::Section);
    let mut fs = Node::new("view-status", "File Status", "gitree-filestatus-symbolic", Kind::View(View::Status));
    let n = s.status.change_count();
    if n > 0 {
        fs.detail = n.to_string();
    }
    ws.children.push(Rc::new(fs));
    ws.children.push(Rc::new(Node::new("view-history", "History", "gitree-history-symbolic", Kind::View(View::History))));
    ws.children.push(Rc::new(Node::new("view-search", "Search", "system-search-symbolic", Kind::View(View::Search))));
    roots.push(Rc::new(ws));

    let mut br = Node::new("branches", "BRANCHES", "", Kind::Section);
    for r in s.refs.locals() {
        let mut leaf = Node::new(format!("b:{}", r.name), r.name.rsplit('/').next().unwrap_or(&r.name), "gitree-branch-symbolic", Kind::Branch(r.clone()));
        leaf.bold = r.is_head;
        let mut d = String::new();
        if r.ahead > 0 {
            d.push_str(&format!("{}↑", r.ahead));
        }
        if r.behind > 0 {
            if !d.is_empty() {
                d.push(' ');
            }
            d.push_str(&format!("{}↓", r.behind));
        }
        if r.upstream_gone {
            d.push_str("gone");
        }
        leaf.detail = d;
        let parts: Vec<&str> = r.name.split('/').collect();
        insert_path(&mut br, "branches", &parts, leaf);
    }
    roots.push(Rc::new(br));

    let mut tags = Node::new("tags", "TAGS", "", Kind::Section);
    let mut tag_list: Vec<&RefInfo> = s.refs.tags().collect();
    tag_list.reverse();
    for t in tag_list {
        tags.children.push(Rc::new(Node::new(format!("t:{}", t.name), t.name.clone(), "gitree-tag-symbolic", Kind::Tag(t.clone()))));
    }
    roots.push(Rc::new(tags));

    let mut remotes = Node::new("remotes", "REMOTES", "", Kind::Section);
    for rem in &s.remotes {
        let mut rn = Node::new(format!("r:{}", rem.name), rem.name.clone(), "gitree-remote-symbolic", Kind::Remote(rem.name.clone()));
        for r in s.refs.remotes().filter(|r| r.remote() == Some(rem.name.as_str())) {
            let b = r.remote_branch().unwrap_or(&r.name);
            let leaf = Node::new(format!("rb:{}", r.name), b.rsplit('/').next().unwrap_or(b), "gitree-branch-symbolic", Kind::RemoteBranch(r.clone()));
            let parts: Vec<&str> = b.split('/').collect();
            let prefix = format!("r:{}", rem.name);
            insert_path(&mut rn, &prefix, &parts, leaf);
        }
        remotes.children.push(Rc::new(rn));
    }
    roots.push(Rc::new(remotes));

    let mut stashes = Node::new("stashes", "STASHES", "", Kind::Section);
    for st in &s.stashes {
        let mut n = Node::new(format!("s:{}", st.oid), st.message.clone(), "gitree-stash-symbolic", Kind::Stash(st.name.clone()));
        n.detail = st.name.trim_start_matches("stash").to_string();
        stashes.children.push(Rc::new(n));
    }
    roots.push(Rc::new(stashes));

    if !s.submodules.is_empty() {
        let mut subs = Node::new("submodules", "SUBMODULES", "", Kind::Section);
        for sm in &s.submodules {
            let mut n = Node::new(format!("sm:{}", sm.path), sm.path.clone(), "gitree-submodule-symbolic", Kind::Submodule(sm.path.clone()));
            n.detail = match sm.state {
                '-' => "not initialised".into(),
                '+' => "modified".into(),
                'U' => "conflict".into(),
                _ => String::new(),
            };
            subs.children.push(Rc::new(n));
        }
        roots.push(Rc::new(subs));
    }
    if !s.subtrees.is_empty() {
        let mut subs = Node::new("subtrees", "SUBTREES", "", Kind::Section);
        for st in &s.subtrees {
            subs.children.push(Rc::new(Node::new(format!("st:{}", st.prefix), st.prefix.clone(), "folder-remote-symbolic", Kind::Subtree(st.prefix.clone()))));
        }
        roots.push(Rc::new(subs));
    }
    roots
}

fn node_of(obj: &glib::Object) -> Option<Rc<Node>> {
    let row = obj.downcast_ref::<gtk::TreeListRow>()?;
    let item = row.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    let n = item.borrow::<Rc<Node>>().clone();
    Some(n)
}

struct Cell_ {
    expander: gtk::TreeExpander,
    icon: gtk::Image,
    label: gtk::Label,
    detail: gtk::Label,
    node: RefCell<Option<Rc<Node>>>,
}

pub struct Sidebar {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListView,
    root: gio::ListStore,
    model: gtk::TreeListModel,
    selection: gtk::SingleSelection,
    rv: Weak<RepoView>,
    expanded: RefCell<HashSet<String>>,
    collapsed: RefCell<HashSet<String>>,
    updating: Cell<bool>,
}

impl Sidebar {
    pub fn new(rv: Weak<RepoView>) -> Rc<Self> {
        let root = gio::ListStore::new::<glib::BoxedAnyObject>();
        let model = gtk::TreeListModel::new(root.clone(), false, false, |obj| {
            let item = obj.downcast_ref::<glib::BoxedAnyObject>()?;
            let node = item.borrow::<Rc<Node>>();
            if node.children.is_empty() {
                return None;
            }
            let store = gio::ListStore::new::<glib::BoxedAnyObject>();
            for c in &node.children {
                store.append(&glib::BoxedAnyObject::new(c.clone()));
            }
            Some(store.upcast())
        });
        let selection = gtk::SingleSelection::builder()
            .model(&model)
            .autoselect(false)
            .can_unselect(true)
            .build();
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(false)
            .build();
        list.add_css_class("navigation-sidebar");
        let widget = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(180)
            .build();
        widget.add_css_class("repo-sidebar");

        let this = Rc::new(Self {
            widget,
            list,
            root,
            model,
            selection,
            rv,
            expanded: RefCell::new(HashSet::new()),
            collapsed: RefCell::new(["tags".to_string()].into_iter().collect()),
            updating: Cell::new(false),
        });

        let weak = Rc::downgrade(&this);
        factory.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let icon = gtk::Image::new();
            let label = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let detail = gtk::Label::new(None);
            detail.add_css_class("dim-label");
            detail.add_css_class("caption");
            hbox.append(&icon);
            hbox.append(&label);
            hbox.append(&detail);
            let expander = gtk::TreeExpander::builder().child(&hbox).build();
            item.set_child(Some(&expander));
            let cell = Rc::new(Cell_ {
                expander: expander.clone(),
                icon,
                label,
                detail,
                node: RefCell::new(None),
            });

            let gesture = gtk::GestureClick::builder().button(3).build();
            let c2 = cell.clone();
            let w2 = weak.clone();
            let e2 = expander.clone();
            gesture.connect_pressed(move |g, _, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                let node = c2.node.borrow().clone();
                if let (Some(n), Some(s)) = (node, w2.upgrade()) {
                    s.context_menu(&e2, x, y, &n);
                }
            });
            expander.add_controller(gesture);
            unsafe {
                item.set_data("cell", cell);
            }
        });
        factory.connect_bind(|_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let cell: Rc<Cell_> = unsafe { item.data::<Rc<Cell_>>("cell").unwrap().as_ref().clone() };
            let row = item.item().and_downcast::<gtk::TreeListRow>();
            cell.expander.set_list_row(row.as_ref());
            let Some(node) = item.item().and_then(|o| node_of(&o)) else { return };
            let section = matches!(node.kind, Kind::Section);
            cell.label.set_text(&node.label);
            cell.icon.set_visible(!node.icon.is_empty());
            cell.icon.set_icon_name(Some(node.icon));
            cell.detail.set_text(&node.detail);
            cell.detail.set_visible(!node.detail.is_empty());
            if section {
                cell.label.add_css_class("sidebar-section");
            } else {
                cell.label.remove_css_class("sidebar-section");
            }
            if node.bold {
                cell.label.add_css_class("current-branch");
            } else {
                cell.label.remove_css_class("current-branch");
            }
            let tip = match &node.kind {
                Kind::Branch(r) => r
                    .upstream
                    .as_ref()
                    .map(|u| format!("{} → {}\n{}", r.name, u, r.subject))
                    .unwrap_or_else(|| format!("{}\n{}", r.name, r.subject)),
                Kind::RemoteBranch(r) | Kind::Tag(r) => format!("{}\n{}", r.name, r.subject),
                Kind::Stash(n) => format!("{n}: {}", node.label),
                _ => String::new(),
            };
            cell.expander.set_tooltip_text((!tip.is_empty()).then_some(tip.as_str()));
            item.set_selectable(!section && !matches!(node.kind, Kind::Folder | Kind::Remote(_)));
            *cell.node.borrow_mut() = Some(node);
        });
        factory.connect_unbind(|_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let cell: Rc<Cell_> = unsafe { item.data::<Rc<Cell_>>("cell").unwrap().as_ref().clone() };
            *cell.node.borrow_mut() = None;
        });

        let weak = Rc::downgrade(&this);
        this.selection.connect_selection_changed(move |sel, _, _| {
            let Some(s) = weak.upgrade() else { return };
            if s.updating.get() {
                return;
            }
            let Some(node) = sel.selected_item().and_then(|o| node_of(&o)) else { return };
            let Some(rv) = s.rv.upgrade() else { return };
            match &node.kind {
                Kind::View(v) => rv.show_view(*v),
                Kind::Branch(r) | Kind::RemoteBranch(r) | Kind::Tag(r) => {
                    rv.jump_to_commit(&r.oid);
                }
                _ => {}
            }
        });

        let weak = Rc::downgrade(&this);
        this.list.connect_activate(move |_, pos| {
            let Some(s) = weak.upgrade() else { return };
            let Some(obj) = s.model.item(pos) else { return };
            let Some(node) = node_of(&obj) else { return };
            let Some(rv) = s.rv.upgrade() else { return };
            let act = |name: &str, arg: &str| super::dialogs::dispatch(&rv, name, arg.to_string());
            match &node.kind {
                Kind::Branch(r) if !r.is_head => act("checkout-ref", &r.full),
                Kind::RemoteBranch(r) => act("checkout-ref", &r.full),
                Kind::Stash(name) => act("stash-show", name),
                Kind::Submodule(p) => act("open-submodule", p),
                Kind::Section | Kind::Folder | Kind::Remote(_) => {
                    if let Some(row) = obj.downcast_ref::<gtk::TreeListRow>() {
                        row.set_expanded(!row.is_expanded());
                    }
                }
                _ => {}
            }
        });

        this
    }

    fn remember_expansion(&self) {
        let mut exp = self.expanded.borrow_mut();
        let mut col = self.collapsed.borrow_mut();
        for i in 0..self.model.n_items() {
            let Some(row) = self.model.row(i) else { continue };
            let Some(item) = row.item().and_downcast::<glib::BoxedAnyObject>() else { continue };
            let n = item.borrow::<Rc<Node>>();
            if n.children.is_empty() {
                continue;
            }
            if row.is_expanded() {
                exp.insert(n.key.clone());
                col.remove(&n.key);
            } else {
                exp.remove(&n.key);
                col.insert(n.key.clone());
            }
        }
    }

    pub fn update(&self, s: &Snapshot) {
        self.updating.set(true);
        let had_items = self.root.n_items() > 0;
        if had_items {
            self.remember_expansion();
        }
        let selected_key = self
            .selection
            .selected_item()
            .and_then(|o| node_of(&o))
            .map(|n| n.key.clone());

        let nodes = build_tree(s);
        let objs: Vec<glib::BoxedAnyObject> = nodes.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.root.splice(0, self.root.n_items(), &objs);

        // Restore expansion: sections open by default, everything else closed
        // unless the user opened it.
        let exp = self.expanded.borrow().clone();
        let col = self.collapsed.borrow().clone();
        let mut i = 0;
        while i < self.model.n_items() {
            if let Some(row) = self.model.row(i)
                && let Some(item) = row.item().and_downcast::<glib::BoxedAnyObject>() {
                    let n = item.borrow::<Rc<Node>>();
                    let open = if matches!(n.kind, Kind::Section) {
                        !col.contains(&n.key)
                    } else {
                        exp.contains(&n.key)
                    };
                    if open && !n.children.is_empty() {
                        drop(n);
                        row.set_expanded(true);
                    }
                }
            i += 1;
        }
        // Restore selection.
        let mut sel = gtk::INVALID_LIST_POSITION;
        if let Some(key) = selected_key {
            for i in 0..self.model.n_items() {
                if let Some(n) = self.model.item(i).and_then(|o| node_of(&o))
                    && n.key == key {
                        sel = i;
                        break;
                    }
            }
        }
        self.selection.set_selected(sel);
        self.updating.set(false);
    }

    /// Highlights the workspace entry for `v`.
    pub fn select_view(&self, v: View) {
        let key = match v {
            View::Status => "view-status",
            View::History => "view-history",
            View::Search => "view-search",
        };
        self.updating.set(true);
        for i in 0..self.model.n_items() {
            if let Some(n) = self.model.item(i).and_then(|o| node_of(&o))
                && n.key == key {
                    self.selection.set_selected(i);
                    break;
                }
        }
        self.updating.set(false);
    }

    fn context_menu(&self, widget: &gtk::TreeExpander, x: f64, y: f64, node: &Node) {
        let Some(rv) = self.rv.upgrade() else { return };
        let snap = rv.snapshot();
        let current = snap.current_branch().unwrap_or("").to_string();
        let menu = gio::Menu::new();
        let sec = || gio::Menu::new();
        match &node.kind {
            Kind::Branch(r) => {
                let s1 = sec();
                if !r.is_head {
                    menu_item_target(&s1, &format!("Checkout {}", r.name), "repo.checkout-ref", &r.full);
                }
                if !r.is_head && !current.is_empty() {
                    menu_item_target(&s1, &format!("Merge {} into {}", r.name, current), "repo.merge-ref", &r.name);
                    menu_item_target(&s1, &format!("Rebase {} onto {}", current, r.name), "repo.rebase-onto", &r.name);
                }
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, "Fetch & Pull…", "repo.pull", "");
                menu_item_target(&s2, "Push to…", "repo.push-branch", &r.name);
                if r.upstream.is_none() {
                    menu_item_target(&s2, "Track Remote Branch…", "repo.track", &r.name);
                } else {
                    menu_item_target(&s2, "Change Tracked Branch…", "repo.track", &r.name);
                }
                menu.append_section(None, &s2);
                let s3 = sec();
                if !r.is_head {
                    menu_item_target(&s3, "Diff Against Current", "repo.diff-ref", &r.name);
                }
                menu_item_target(&s3, "Rename…", "repo.rename-branch", &r.name);
                if !r.is_head {
                    menu_item_target(&s3, &format!("Delete {}…", r.name), "repo.delete-branch", &r.name);
                }
                menu_item_target(&s3, "Copy Branch Name", "repo.copy-text", &r.name);
                menu.append_section(None, &s3);
                if let Some(flow) = &snap.flow
                    && flow.classify(&r.name).is_some() {
                        let s4 = sec();
                        menu_item_target(&s4, "Git-flow: Finish…", "repo.flow-finish", &r.name);
                        menu.append_section(None, &s4);
                    }
            }
            Kind::RemoteBranch(r) => {
                let s1 = sec();
                menu_item_target(&s1, &format!("Checkout {}…", r.name), "repo.checkout-ref", &r.full);
                if !current.is_empty() {
                    menu_item_target(&s1, &format!("Pull {} into {}", r.name, current), "repo.pull-ref", &r.name);
                    menu_item_target(&s1, &format!("Merge {} into {}", r.name, current), "repo.merge-ref", &r.name);
                    menu_item_target(&s1, &format!("Rebase {} onto {}", current, r.name), "repo.rebase-onto", &r.name);
                }
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, "Diff Against Current", "repo.diff-ref", &r.name);
                menu_item_target(&s2, "Copy Branch Name", "repo.copy-text", &r.name);
                menu_item_target(&s2, &format!("Delete {}…", r.name), "repo.delete-remote-branch", &r.name);
                menu.append_section(None, &s2);
            }
            Kind::Tag(r) => {
                let s1 = sec();
                menu_item_target(&s1, &format!("Checkout {}", r.name), "repo.checkout-commit", &r.oid);
                menu_item_target(&s1, "Push Tag to…", "repo.push-tag", &r.name);
                menu_item_target(&s1, "Copy Tag Name", "repo.copy-text", &r.name);
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, &format!("Delete {}…", r.name), "repo.delete-tag", &r.name);
                menu.append_section(None, &s2);
            }
            Kind::Remote(name) => {
                let s1 = sec();
                menu_item_target(&s1, &format!("Fetch from {name}"), "repo.remote-fetch", name);
                menu_item_target(&s1, &format!("Prune stale branches of {name}"), "repo.remote-prune", name);
                menu_item_target(&s1, "Open in Browser", "repo.open-remote", name);
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, "Edit Remote…", "repo.remote-edit", name);
                menu_item_target(&s2, "Remove Remote…", "repo.remote-remove", name);
                menu_item_target(&s2, "Copy URL", "repo.remote-copy-url", name);
                menu.append_section(None, &s2);
            }
            Kind::Stash(name) => {
                let s1 = sec();
                menu_item_target(&s1, "Show Changes", "repo.stash-show", name);
                menu_item_target(&s1, "Apply Stash…", "repo.stash-apply", name);
                menu_item_target(&s1, "Pop Stash", "repo.stash-pop", name);
                menu_item_target(&s1, "Create Branch from Stash…", "repo.stash-branch", name);
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, "Delete Stash…", "repo.stash-drop", name);
                menu.append_section(None, &s2);
            }
            Kind::Submodule(p) => {
                let s1 = sec();
                menu_item_target(&s1, "Open Submodule", "repo.open-submodule", p);
                menu_item_target(&s1, "Update (init, recursive)", "repo.update-submodule", p);
                menu_item_target(&s1, "Sync URL", "repo.sync-submodule", p);
                menu_item_target(&s1, "Show in Files", "repo.file-show", p);
                menu.append_section(None, &s1);
                let s2 = sec();
                menu_item_target(&s2, "Remove Submodule…", "repo.remove-submodule", p);
                menu.append_section(None, &s2);
            }
            Kind::Subtree(p) => {
                let s1 = sec();
                menu_item_target(&s1, "Pull Subtree", "repo.subtree-pull", p);
                menu_item_target(&s1, "Push Subtree", "repo.subtree-push", p);
                menu_item_target(&s1, "Unlink Subtree", "repo.subtree-unlink", p);
                menu.append_section(None, &s1);
            }
            Kind::Section => match node.key.as_str() {
                "branches" => {
                    let s1 = sec();
                    menu_item_target(&s1, "New Branch…", "repo.branch", "");
                    menu.append_section(None, &s1);
                }
                "tags" => {
                    let s1 = sec();
                    menu_item_target(&s1, "New Tag…", "repo.tag", "");
                    menu_item_target(&s1, "Push All Tags…", "repo.push-tag", "");
                    menu.append_section(None, &s1);
                }
                "remotes" => {
                    let s1 = sec();
                    menu_item_target(&s1, "New Remote…", "repo.remote-add", "");
                    menu_item_target(&s1, "Fetch All", "repo.fetch", "");
                    menu.append_section(None, &s1);
                }
                "stashes" => {
                    let s1 = sec();
                    menu_item_target(&s1, "Stash Changes…", "repo.stash", "");
                    menu.append_section(None, &s1);
                }
                "submodules" => {
                    let s1 = sec();
                    menu_item_target(&s1, "Add Submodule…", "repo.add-submodule", "");
                    menu_item_target(&s1, "Update All Submodules", "repo.update-submodule", "");
                    menu.append_section(None, &s1);
                }
                _ => return,
            },
            _ => return,
        }
        popup_menu(widget, x, y, &menu);
    }
}
