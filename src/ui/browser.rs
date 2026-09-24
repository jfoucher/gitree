//! Repository browser (Sourcetree's bookmarks window): lists bookmarked
//! local repositories and offers Clone / Add / Create / Scan.

use super::form::Form;
use super::progress::{self, OpOptions};
use super::{ask_text, bg, confirm, spawn};
use crate::config::{self, Bookmark};
use crate::git::{self, Git};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub type OpenFn = Rc<dyn Fn(PathBuf)>;
type BrowserAction = Box<dyn Fn(&Rc<Browser>, String)>;

pub struct Browser {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    on_open: OpenFn,
    rows: RefCell<Vec<(gtk::ListBoxRow, Bookmark, gtk::Label)>>,
}

impl Browser {
    pub fn new(on_open: OpenFn) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("repo-toolbar");
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search repositories")
            .hexpand(true)
            .build();
        bar.append(&search);

        let new_menu = gio::Menu::new();
        new_menu.append(Some("Clone from URL…"), Some("browser.clone"));
        new_menu.append(Some("Add Existing Local Repository…"), Some("browser.add"));
        new_menu.append(Some("Create Local Repository…"), Some("browser.create"));
        new_menu.append(Some("Scan a Folder for Repositories…"), Some("browser.scan"));
        let new_btn = gtk::MenuButton::builder()
            .label("New…")
            .menu_model(&new_menu)
            .build();
        new_btn.add_css_class("suggested-action");
        bar.append(&new_btn);
        widget.append(&bar);

        let list = gtk::ListBox::new();
        list.add_css_class("repo-browser");
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::Single);
        let sw = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let clamp = adw::Clamp::builder().maximum_size(900).child(&sw).build();
        clamp.set_vexpand(true);
        let status_page = adw::StatusPage::builder()
            .icon_name("gitree-repo-symbolic")
            .title("No Repositories")
            .description("Clone a remote repository, add an existing local one, or create a new one using the “New…” menu.")
            .vexpand(true)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&clamp, Some("list"));
        stack.add_named(&status_page, Some("empty"));
        widget.append(&stack);

        let this = Rc::new(Self {
            widget,
            list,
            search,
            on_open,
            rows: RefCell::new(Vec::new()),
        });

        this.setup_actions();

        let t = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            if let Some(t) = t.upgrade() {
                let path = t
                    .rows
                    .borrow()
                    .iter()
                    .find(|(r, _, _)| r == row)
                    .map(|(_, b, _)| b.path.clone());
                if let Some(p) = path {
                    if p.exists() {
                        (t.on_open)(p);
                    } else {
                        super::show_error(&t.widget, "Repository not found", &p.to_string_lossy());
                    }
                }
            }
        });

        let t = Rc::downgrade(&this);
        this.search.connect_search_changed(move |_| {
            if let Some(t) = t.upgrade() {
                t.list.invalidate_filter();
            }
        });
        let t = Rc::downgrade(&this);
        this.list.set_filter_func(move |row| {
            let Some(t) = t.upgrade() else { return true };
            let q = t.search.text().to_lowercase();
            if q.is_empty() {
                return true;
            }
            t.rows
                .borrow()
                .iter()
                .find(|(r, _, _)| r == row)
                .map(|(_, b, _)| {
                    b.name.to_lowercase().contains(&q)
                        || b.path.to_string_lossy().to_lowercase().contains(&q)
                })
                .unwrap_or(true)
        });
        let t = Rc::downgrade(&this);
        this.list.set_header_func(move |row, before| {
            let Some(t) = t.upgrade() else { return };
            let rows = t.rows.borrow();
            let group_of = |r: &gtk::ListBoxRow| {
                rows.iter()
                    .find(|(x, _, _)| x == r)
                    .map(|(_, b, _)| b.group.clone())
                    .unwrap_or_default()
            };
            let g = group_of(row);
            let prev = before.map(group_of);
            if !g.is_empty() && prev.as_deref() != Some(g.as_str()) {
                let l = gtk::Label::builder().label(&g).xalign(0.0).build();
                l.add_css_class("group-header");
                row.set_header(Some(&l));
            } else if g.is_empty() && prev.as_ref().is_some_and(|p| !p.is_empty()) {
                let l = gtk::Label::builder().label("Ungrouped").xalign(0.0).build();
                l.add_css_class("group-header");
                row.set_header(Some(&l));
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });

        this.reload_into(&stack);
        this
    }

    fn setup_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let add = |name: &str, param: bool, f: BrowserAction| {
            let a = gio::SimpleAction::new(name, param.then_some(glib::VariantTy::STRING));
            let t = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                if let Some(t) = t.upgrade() {
                    f(&t, v.and_then(|v| v.get::<String>()).unwrap_or_default());
                }
            });
            group.add_action(&a);
        };
        add(
            "clone",
            false,
            Box::new(|t, _| {
                let t2 = t.clone();
                clone_dialog(&t.widget.clone().upcast(), "", move |p| (t2.on_open)(p));
            }),
        );
        add(
            "add",
            false,
            Box::new(|t, _| {
                let t = t.clone();
                spawn(async move {
                    let t2 = t.clone();
                    add_existing(&t.widget.clone().upcast(), move |p| {
                        t2.reload();
                        (t2.on_open)(p)
                    })
                    .await;
                });
            }),
        );
        add(
            "create",
            false,
            Box::new(|t, _| {
                let t = t.clone();
                spawn(async move { t.create_repo().await });
            }),
        );
        add(
            "scan",
            false,
            Box::new(|t, _| {
                let t = t.clone();
                spawn(async move { t.scan().await });
            }),
        );
        add(
            "open",
            true,
            Box::new(|t, p| (t.on_open)(PathBuf::from(p))),
        );
        add(
            "show",
            true,
            Box::new(|_, p| super::open_folder(Path::new(&p))),
        );
        add(
            "terminal",
            true,
            Box::new(|t, p| {
                if let Err(e) = super::open_terminal(Path::new(&p)) {
                    super::show_error(&t.widget, "Could not open terminal", &e);
                }
            }),
        );
        add(
            "rename",
            true,
            Box::new(|t, p| {
                let t = t.clone();
                spawn(async move {
                    let cur = config::with(|s| {
                        s.bookmarks
                            .iter()
                            .find(|b| b.path == Path::new(&p))
                            .map(|b| b.name.clone())
                    })
                    .unwrap_or_default();
                    if let Some(name) = ask_text(&t.widget, "Rename Bookmark", "", &cur, "Rename").await {
                        config::update(|s| {
                            if let Some(b) = s.bookmarks.iter_mut().find(|b| b.path == Path::new(&p)) {
                                b.name = name;
                            }
                        });
                        t.reload();
                    }
                });
            }),
        );
        add(
            "group",
            true,
            Box::new(|t, p| {
                let t = t.clone();
                spawn(async move {
                    let cur = config::with(|s| {
                        s.bookmarks
                            .iter()
                            .find(|b| b.path == Path::new(&p))
                            .map(|b| b.group.clone())
                    })
                    .unwrap_or_default();
                    let d = adw::AlertDialog::new(
                        Some("Move to Group"),
                        Some("Enter a group (folder) name, or leave empty to ungroup."),
                    );
                    let e = gtk::Entry::builder().text(&cur).activates_default(true).build();
                    d.set_extra_child(Some(&e));
                    d.add_responses(&[("cancel", "Cancel"), ("ok", "Move")]);
                    d.set_default_response(Some("ok"));
                    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
                    if d.choose_future(Some(&t.widget)).await == "ok" {
                        let g = e.text().trim().to_string();
                        config::update(|s| {
                            if let Some(b) = s.bookmarks.iter_mut().find(|b| b.path == Path::new(&p)) {
                                b.group = g;
                            }
                        });
                        t.reload();
                    }
                });
            }),
        );
        add(
            "remove",
            true,
            Box::new(|t, p| {
                let t = t.clone();
                spawn(async move {
                    if confirm(
                        &t.widget,
                        "Remove Bookmark?",
                        "The repository stays on disk; only the bookmark is removed.",
                        "Remove",
                        true,
                    )
                    .await
                    {
                        config::update(|s| s.bookmarks.retain(|b| b.path != Path::new(&p)));
                        t.reload();
                    }
                });
            }),
        );
        self.widget.insert_action_group("browser", Some(&group));
    }

    pub fn reload(&self) {
        if let Some(stack) = self.widget.last_child().and_downcast::<gtk::Stack>() {
            self.reload_into(&stack);
        }
    }

    fn reload_into(&self, stack: &gtk::Stack) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let mut bookmarks = config::with(|s| s.bookmarks.clone());
        // Group order: ungrouped first, then alphabetical groups.
        bookmarks.sort_by_key(|a| a.group.to_lowercase());
        let mut rows = Vec::new();
        for b in &bookmarks {
            let (row, info) = self.make_row(b);
            self.list.append(&row);
            rows.push((row, b.clone(), info));
        }
        *self.rows.borrow_mut() = rows;
        stack.set_visible_child_name(if bookmarks.is_empty() { "empty" } else { "list" });
        self.list.invalidate_headers();

        // Fill in branch + change counts in the background.
        let paths: Vec<(usize, PathBuf)> = bookmarks
            .iter()
            .enumerate()
            .map(|(i, b)| (i, b.path.clone()))
            .collect();
        let labels: Vec<gtk::Label> = self.rows.borrow().iter().map(|(_, _, l)| l.clone()).collect();
        spawn(async move {
            let infos = bg(move || {
                paths
                    .into_iter()
                    .map(|(i, p)| {
                        if !p.exists() {
                            return (i, "Missing".to_string());
                        }
                        match git::status::status(&Git::new(&p), false) {
                            Ok(st) => {
                                let branch = st.branch.clone().unwrap_or_else(|| "detached HEAD".into());
                                let n = st.change_count();
                                let mut s = branch;
                                if st.ahead > 0 {
                                    s.push_str(&format!("  ↑{}", st.ahead));
                                }
                                if st.behind > 0 {
                                    s.push_str(&format!("  ↓{}", st.behind));
                                }
                                if n > 0 {
                                    s.push_str(&format!("  •  {n} changed"));
                                }
                                (i, s)
                            }
                            Err(_) => (i, "Not a repository".into()),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            for (i, s) in infos {
                if let Some(l) = labels.get(i) {
                    l.set_text(&s);
                }
            }
        });
    }

    fn make_row(&self, b: &Bookmark) -> (gtk::ListBoxRow, gtk::Label) {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_start(6);
        hbox.set_margin_end(6);
        let icon = gtk::Image::from_icon_name("gitree-repo-symbolic");
        icon.set_pixel_size(24);
        hbox.append(&icon);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);
        let name = gtk::Label::builder().label(&b.name).xalign(0.0).build();
        name.add_css_class("repo-name");
        let path = gtk::Label::builder()
            .label(b.path.to_string_lossy().as_ref())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        path.add_css_class("repo-path");
        vbox.append(&name);
        vbox.append(&path);
        hbox.append(&vbox);
        let info = gtk::Label::builder().label("").xalign(1.0).build();
        info.add_css_class("dim-label");
        hbox.append(&info);
        let row = gtk::ListBoxRow::builder().child(&hbox).build();

        let p = b.path.to_string_lossy().to_string();
        let gesture = gtk::GestureClick::builder().button(3).build();
        let r = row.clone();
        gesture.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            let menu = gio::Menu::new();
            let s1 = gio::Menu::new();
            super::menu_item_target(&s1, "Open", "browser.open", &p);
            super::menu_item_target(&s1, "Show in Files", "browser.show", &p);
            super::menu_item_target(&s1, "Open in Terminal", "browser.terminal", &p);
            menu.append_section(None, &s1);
            let s2 = gio::Menu::new();
            super::menu_item_target(&s2, "Rename…", "browser.rename", &p);
            super::menu_item_target(&s2, "Move to Group…", "browser.group", &p);
            super::menu_item_target(&s2, "Remove Bookmark", "browser.remove", &p);
            menu.append_section(None, &s2);
            super::popup_menu(&r, x, y, &menu);
        });
        row.add_controller(gesture);
        (row, info)
    }

    async fn create_repo(self: &Rc<Self>) {
        let form = Form::new("Create a Repository", "Create");
        let default = config::with(|s| s.default_clone_dir.clone())
            .unwrap_or_default()
            .join("new-repo");
        let path = form.entry("Destination path", &default.to_string_lossy());
        add_browse_button(&form, &path);
        let branch = form.entry("Initial branch", "main");
        let bare = form.switch("Bare repository", "No working copy (for use as a remote)", false);
        form.watch(&path);
        let p2 = path.clone();
        form.validate(move || !p2.text().trim().is_empty());
        if !form.run(&self.widget).await {
            return;
        }
        let dest = PathBuf::from(expand_tilde(path.text().trim()));
        if let Err(e) = std::fs::create_dir_all(&dest) {
            super::show_error(&self.widget, "Could not create folder", &e.to_string());
            return;
        }
        let mut args = vec!["init".to_string()];
        if !branch.text().trim().is_empty() {
            args.push(format!("--initial-branch={}", branch.text().trim()));
        }
        if bare.is_active() {
            args.push("--bare".into());
        }
        let ok = progress::run(
            &self.widget.clone().upcast(),
            &Git::new(&dest),
            "Create Repository",
            vec![args],
            OpOptions::default(),
        )
        .await
        .is_ok();
        if ok && !bare.is_active() {
            config::update(|s| s.add_bookmark(dest.clone()));
            self.reload();
            (self.on_open)(dest);
        }
    }

    async fn scan(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose a folder to scan for repositories")
            .build();
        let win = self.widget.root().and_downcast::<gtk::Window>();
        let Ok(folder) = dialog.select_folder_future(win.as_ref()).await else {
            return;
        };
        let Some(root) = folder.path() else { return };
        let group = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let found = bg(move || {
            let mut out = Vec::new();
            scan_dir(&root, 0, &mut out);
            out
        })
        .await;
        let n = found.len();
        config::update(|s| {
            for p in found {
                if !s.bookmarks.iter().any(|b| b.path == p) {
                    s.add_bookmark(p.clone());
                    if let Some(b) = s.bookmarks.last_mut() {
                        b.group = group.clone();
                    }
                }
            }
        });
        self.reload();
        super::toast(&self.widget, &format!("Found {n} repositories"));
    }
}

fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth >= 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let hidden = p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if p.is_dir() && !hidden && !p.ends_with("node_modules") {
            scan_dir(&p, depth + 1, out);
        }
    }
}

pub fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(h) = dirs::home_dir() {
            return h.join(rest).to_string_lossy().into_owned();
        }
    p.to_string()
}

/// Adds a "Browse…" suffix button to an entry row that picks a folder.
pub fn add_browse_button(form: &Rc<Form>, entry: &adw::EntryRow) {
    let btn = gtk::Button::from_icon_name("folder-open-symbolic");
    btn.set_valign(gtk::Align::Center);
    btn.add_css_class("flat");
    btn.set_tooltip_text(Some("Browse…"));
    entry.add_suffix(&btn);
    let e = entry.clone();
    let d = form.dialog.clone();
    btn.connect_clicked(move |_| {
        let e = e.clone();
        let win = d.root().and_downcast::<gtk::Window>();
        spawn(async move {
            let fd = gtk::FileDialog::builder().title("Choose Folder").build();
            let cur = PathBuf::from(expand_tilde(e.text().trim()));
            let start = if cur.is_dir() {
                Some(cur)
            } else {
                cur.parent().map(Path::to_path_buf)
            };
            if let Some(s) = start.filter(|s| s.is_dir()) {
                fd.set_initial_folder(Some(&gio::File::for_path(s)));
            }
            if let Ok(f) = fd.select_folder_future(win.as_ref()).await
                && let Some(p) = f.path() {
                    e.set_text(&p.to_string_lossy());
                }
        });
    });
}

/// Name of the repository a clone URL points to.
pub fn repo_name_from_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    let last = u.rsplit(['/', ':']).next().unwrap_or("");
    last.trim_end_matches(".git").to_string()
}

/// "Add existing local repository" flow.
pub async fn add_existing(parent: &gtk::Widget, on_open: impl Fn(PathBuf) + 'static) {
    let dialog = gtk::FileDialog::builder()
        .title("Choose a Local Repository")
        .build();
    let win = parent.root().and_downcast::<gtk::Window>();
    let Ok(folder) = dialog.select_folder_future(win.as_ref()).await else {
        return;
    };
    let Some(path) = folder.path() else { return };
    let p2 = path.clone();
    match bg(move || git::find_toplevel(&p2)).await {
        Some(top) => {
            config::update(|s| s.add_bookmark(top.clone()));
            on_open(top);
        }
        None => {
            let p3 = path.clone();
            if confirm(
                parent,
                "Not a Git Repository",
                &format!("{}\n\nDo you want to initialise a new repository here?", path.display()),
                "Create Repository",
                false,
            )
            .await
                && Git::new(&p3).run(&["init"]).is_ok()
            {
                config::update(|s| s.add_bookmark(p3.clone()));
                on_open(p3);
            }
        }
    }
}

/// The "Clone" sheet.
pub fn clone_dialog(parent: &gtk::Widget, url: &str, on_open: impl Fn(PathBuf) + 'static) {
    let parent = parent.clone();
    let url = url.to_string();
    spawn(async move {
        let form = Form::new("Clone a Repository", "Clone");
        let src = form.entry("Source URL", &url);
        let base = config::with(|s| s.default_clone_dir.clone()).unwrap_or_default();
        let dest = form.entry("Destination path", &base.to_string_lossy());
        add_browse_button(&form, &dest);
        let name = form.entry("Bookmark name", "");
        form.group("Advanced Options");
        let branch = form.entry("Checkout branch (optional)", "");
        let depth = form.spin("Clone depth (0 = full history)", 0.0, 100000.0, 0.0);
        let recurse = form.switch("Recurse submodules", "", true);
        let lfs = form.switch("Skip LFS files during clone", "Use `git lfs pull` later", false);

        // Auto-fill destination and name from the URL.
        let (d2, n2) = (dest.clone(), name.clone());
        let base2 = base.clone();
        let last_auto = Rc::new(RefCell::new(String::new()));
        src.connect_changed(move |e| {
            let repo = repo_name_from_url(&e.text());
            if repo.is_empty() {
                return;
            }
            let cur = d2.text().to_string();
            let auto = last_auto.borrow().clone();
            if cur == base2.to_string_lossy() || cur == auto || cur.is_empty() {
                let new = base2.join(&repo).to_string_lossy().to_string();
                d2.set_text(&new);
                *last_auto.borrow_mut() = new;
            }
            n2.set_text(&repo);
        });
        form.watch(&src);
        form.watch(&dest);
        let (s3, d3) = (src.clone(), dest.clone());
        form.validate(move || {
            let dp = PathBuf::from(expand_tilde(d3.text().trim()));
            !s3.text().trim().is_empty()
                && !d3.text().trim().is_empty()
                && !(dp.exists() && std::fs::read_dir(&dp).map(|mut r| r.next().is_some()).unwrap_or(false))
        });
        if !url.is_empty() {
            src.emit_by_name::<()>("changed", &[]);
        }
        if !form.run(&parent).await {
            return;
        }

        let dest_path = PathBuf::from(expand_tilde(dest.text().trim()));
        let Some(dest_parent) = dest_path.parent().map(Path::to_path_buf) else {
            return;
        };
        let _ = std::fs::create_dir_all(&dest_parent);
        let mut args = vec!["clone".to_string(), "--progress".to_string()];
        if !branch.text().trim().is_empty() {
            args.push("--branch".into());
            args.push(branch.text().trim().to_string());
        }
        if depth.value() > 0.0 {
            args.push("--depth".into());
            args.push((depth.value() as u32).to_string());
        }
        if recurse.is_active() {
            args.push("--recurse-submodules".into());
        }
        args.push(src.text().trim().to_string());
        args.push(dest_path.to_string_lossy().to_string());
        let mut opts = OpOptions {
            network: true,
            ..Default::default()
        };
        if lfs.is_active() {
            opts.env.push(("GIT_LFS_SKIP_SMUDGE".into(), "1".into()));
        }
        let ok = progress::run(&parent, &Git::new(&dest_parent), "Clone", vec![args], opts)
            .await
            .is_ok();
        if ok {
            let bname = name.text().trim().to_string();
            config::update(|s| {
                s.add_bookmark(dest_path.clone());
                if !bname.is_empty()
                    && let Some(b) = s.bookmarks.iter_mut().find(|b| b.path == dest_path) {
                        b.name = bname;
                    }
            });
            on_open(dest_path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names() {
        assert_eq!(repo_name_from_url("https://github.com/a/b.git"), "b");
        assert_eq!(repo_name_from_url("git@github.com:a/foo-bar.git"), "foo-bar");
        assert_eq!(repo_name_from_url("/srv/git/thing/"), "thing");
    }
}
