//! Main window: a tab per repository plus the repository browser.

use super::browser::Browser;
use super::repo_view::RepoView;
use crate::config;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

type WindowAction = Box<dyn Fn(&Rc<MainWindow>)>;

enum PageKind {
    Browser(Rc<Browser>),
    Repo(Rc<RepoView>),
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    tabs: adw::TabView,
    pages: RefCell<Vec<(adw::TabPage, PageKind)>>,
    title: adw::WindowTitle,
    self_ref: RefCell<Weak<MainWindow>>,
}

thread_local! {
    static WINDOW: RefCell<Option<Rc<MainWindow>>> = const { RefCell::new(None) };
}

/// App-level actions and keyboard shortcuts.
pub fn setup_app(app: &adw::Application) {
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.clone();
    quit.connect_activate(move |_, _| {
        for w in a.windows() {
            w.close();
        }
    });
    app.add_action(&quit);

    let prefs = gio::SimpleAction::new("preferences", None);
    let a = app.clone();
    prefs.connect_activate(move |_, _| {
        if let Some(w) = a.active_window() {
            super::preferences::show(&w);
        }
    });
    app.add_action(&prefs);

    let about = gio::SimpleAction::new("about", None);
    let a = app.clone();
    about.connect_activate(move |_, _| {
        let d = adw::AboutDialog::builder()
            .application_name("Gitree")
            .application_icon(crate::APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A Git client for GNOME inspired by Sourcetree")
            .license_type(gtk::License::Gpl30)
            .build();
        d.present(a.active_window().as_ref());
    });
    app.add_action(&about);

    let accels: &[(&str, &[&str])] = &[
        ("app.quit", &["<Ctrl>q"]),
        ("app.preferences", &["<Ctrl>comma"]),
        ("win.new-tab", &["<Ctrl>t"]),
        ("win.close-tab", &["<Ctrl>w"]),
        ("win.open", &["<Ctrl>o"]),
        ("win.clone", &["<Ctrl><Shift>n"]),
        ("win.next-tab", &["<Ctrl>Page_Down", "<Ctrl>Tab"]),
        ("win.prev-tab", &["<Ctrl>Page_Up", "<Ctrl><Shift>Tab"]),
        ("repo.commit('')", &["<Ctrl><Shift>c"]),
        ("repo.pull('')", &["<Ctrl><Shift>l"]),
        ("repo.push('')", &["<Ctrl><Shift>p"]),
        ("repo.fetch('')", &["<Ctrl><Shift>f"]),
        ("repo.branch('')", &["<Ctrl><Shift>b"]),
        ("repo.merge('')", &["<Ctrl><Shift>m"]),
        ("repo.stash('')", &["<Ctrl><Shift>s"]),
        ("repo.tag('')", &["<Ctrl><Shift>t"]),
        ("repo.discard('')", &["<Ctrl><Shift>r"]),
        ("repo.show-status('')", &["<Ctrl>1"]),
        ("repo.show-history('')", &["<Ctrl>2"]),
        ("repo.show-search('')", &["<Ctrl>3", "<Ctrl>f"]),
        ("repo.refresh('')", &["<Ctrl>r", "F5"]),
        ("repo.terminal('')", &["<Ctrl><Alt>t"]),
        ("repo.files('')", &["<Ctrl><Alt>o"]),
        ("repo.settings('')", &["<Ctrl><Shift>comma"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
}

impl MainWindow {
    pub fn get_or_create(app: &adw::Application) -> Rc<MainWindow> {
        if let Some(w) = WINDOW.with(|w| w.borrow().clone()) {
            return w;
        }
        let w = Self::new(app);
        WINDOW.with(|cell| *cell.borrow_mut() = Some(w.clone()));
        w
    }

    fn new(app: &adw::Application) -> Rc<Self> {
        let (width, height, maximized) =
            config::with(|s| (s.window_width, s.window_height, s.window_maximized));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Gitree")
            .default_width(width)
            .default_height(height)
            .maximized(maximized)
            .build();
        window.set_size_request(760, 480);

        let tabs = adw::TabView::new();
        let tab_bar = adw::TabBar::builder().view(&tabs).autohide(false).build();

        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new("Gitree", "");
        header.set_title_widget(Some(&title));

        let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab.set_tooltip_text(Some("New Tab (Ctrl+T)"));
        new_tab.set_action_name(Some("win.new-tab"));
        header.pack_start(&new_tab);

        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("New Tab"), Some("win.new-tab"));
        s1.append(Some("Open Repository…"), Some("win.open"));
        s1.append(Some("Clone Repository…"), Some("win.clone"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some("Preferences"), Some("app.preferences"));
        s2.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        s2.append(Some("About Gitree"), Some("app.about"));
        menu.append_section(None, &s2);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_btn);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.add_top_bar(&tab_bar);
        tv.set_content(Some(&tabs));
        window.set_content(Some(&tv));

        let this = Rc::new(Self {
            window,
            tabs,
            pages: RefCell::new(Vec::new()),
            title,
            self_ref: RefCell::new(Weak::new()),
        });
        *this.self_ref.borrow_mut() = Rc::downgrade(&this);
        this.setup_actions();
        this.setup_signals();

        let tabs_to_open: Vec<PathBuf> = config::with(|s| s.open_tabs.clone());
        for p in tabs_to_open {
            if p.exists() {
                this.open_repo(&p);
            }
        }
        if this.tabs.n_pages() == 0 {
            this.new_browser_tab();
        }
        if this.tabs.n_pages() > 0 {
            this.tabs.set_selected_page(&this.tabs.nth_page(0));
        }
        this
    }

    pub fn present(&self) {
        self.window.present();
        if let Ok(path) = std::env::var("GITREE_SCREENSHOT") {
            self.debug_screenshot(path);
        }
    }

    /// Development aid: runs `GITREE_DEBUG_ACTIONS` ("name=arg;..."; the
    /// pseudo-action `wait=<ms>` pauses between actions), renders the
    /// window to a PNG and quits. Used with the broadway backend to check
    /// the UI headlessly.
    fn debug_screenshot(&self, path: String) {
        let win = self.window.clone();
        let w = self.weak();
        super::spawn(async move {
            glib::timeout_future(std::time::Duration::from_millis(1500)).await;
            if let (Some(w), Ok(actions)) = (w.upgrade(), std::env::var("GITREE_DEBUG_ACTIONS"))
                && let Some(rv) = w.current_repo() {
                    for a in actions.split(';').filter(|a| !a.is_empty()) {
                        let (name, arg) = a.split_once('=').unwrap_or((a, ""));
                        if name == "wait" {
                            let ms = arg.parse().unwrap_or(500);
                            glib::timeout_future(std::time::Duration::from_millis(ms)).await;
                        } else {
                            super::dialogs::dispatch(&rv, name, arg.to_string());
                        }
                    }
                }
            let delay = std::env::var("GITREE_SCREENSHOT_DELAY")
                .ok()
                .and_then(|d| d.parse().ok())
                .unwrap_or(2000);
            let tries = std::cell::Cell::new(0);
            let main_win = win.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(delay), move || {
                // Capture the most recently opened window (e.g. Blame).
                let win: gtk::Window = gtk::Window::list_toplevels()
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Window>().ok())
                    .filter(|w| w.is_visible())
                    .find(|w| w != main_win.upcast_ref::<gtk::Window>())
                    .unwrap_or_else(|| main_win.clone().upcast());
                let paintable = gtk::WidgetPaintable::new(Some(&win));
                let (wd, ht) = (win.width() as f64, win.height() as f64);
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, wd, ht);
                match (snap.to_node(), win.renderer()) {
                    (Some(node), Some(renderer)) => {
                        let tex = renderer.render_texture(&node, None);
                        if let Err(e) = tex.save_to_png(&path) {
                            eprintln!("gitree: screenshot failed: {e}");
                        }
                    }
                    _ if tries.get() < 20 => {
                        eprintln!("gitree: no frame yet (mapped={}, {}x{})", win.is_mapped(), wd, ht);
                        // Nothing drawn yet; force a frame and retry.
                        tries.set(tries.get() + 1);
                        win.queue_draw();
                        return glib::ControlFlow::Continue;
                    }
                    _ => eprintln!("gitree: nothing to render"),
                }
                if std::env::var("GITREE_KEEP_OPEN").is_err()
                    && let Some(a) = main_win.application() {
                        a.quit();
                    }
                glib::ControlFlow::Break
            });
        });
    }

    fn weak(&self) -> Weak<MainWindow> {
        self.self_ref.borrow().clone()
    }

    fn setup_actions(&self) {
        let add = |name: &str, f: WindowAction| {
            let a = gio::SimpleAction::new(name, None);
            let w = self.weak();
            a.connect_activate(move |_, _| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&a);
        };
        add("new-tab", Box::new(|w| w.new_browser_tab()));
        add(
            "close-tab",
            Box::new(|w| {
                if let Some(p) = w.tabs.selected_page() {
                    w.tabs.close_page(&p);
                }
            }),
        );
        add(
            "next-tab",
            Box::new(|w| {
                w.tabs.select_next_page();
            }),
        );
        add(
            "prev-tab",
            Box::new(|w| {
                w.tabs.select_previous_page();
            }),
        );
        add(
            "open",
            Box::new(|w| {
                let w = w.clone();
                super::spawn(async move {
                    super::browser::add_existing(&w.window.clone().upcast(), {
                        let w = w.clone();
                        move |p| w.open_path(&p)
                    })
                    .await;
                });
            }),
        );
        add(
            "clone",
            Box::new(|w| {
                let w2 = w.clone();
                super::browser::clone_dialog(&w.window.clone().upcast(), "", move |p| w2.open_path(&p));
            }),
        );
        add("shortcuts", Box::new(|w| show_shortcuts(&w.window)));
    }

    fn setup_signals(&self) {
        let w = self.weak();
        self.tabs.connect_close_page(move |tabs, page| {
            if let Some(w) = w.upgrade() {
                w.pages.borrow_mut().retain(|(p, _)| p != page);
                tabs.close_page_finish(page, true);
                if tabs.n_pages() == 0 {
                    // Never leave the window empty: show the browser.
                    let w2 = w.clone();
                    glib::idle_add_local_once(move || {
                        if w2.tabs.n_pages() == 0 {
                            w2.new_browser_tab();
                        }
                    });
                }
                w.save_tabs();
            }
            glib::Propagation::Stop
        });

        let w = self.weak();
        self.tabs.connect_selected_page_notify(move |_| {
            if let Some(w) = w.upgrade() {
                w.on_page_changed();
            }
        });

        let w = self.weak();
        self.window.connect_is_active_notify(move |win| {
            if win.is_active()
                && let Some(w) = w.upgrade()
                    && let Some(rv) = w.current_repo() {
                        rv.refresh();
                    }
        });

        let w = self.weak();
        self.window.connect_close_request(move |win| {
            if let Some(w) = w.upgrade() {
                w.save_tabs();
            }
            let (width, height) = win.default_size();
            let maximized = win.is_maximized();
            config::update(|s| {
                if !maximized {
                    s.window_width = width;
                    s.window_height = height;
                }
                s.window_maximized = maximized;
            });
            WINDOW.with(|cell| *cell.borrow_mut() = None);
            glib::Propagation::Proceed
        });
    }

    fn on_page_changed(&self) {
        match self.current_repo() {
            Some(rv) => {
                self.title.set_title(&rv.name);
                let path = rv.git.workdir.to_string_lossy().to_string();
                let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_default();
                let short = match path.strip_prefix(&home) {
                    Some(rest) if !home.is_empty() => format!("~{rest}"),
                    _ => path,
                };
                self.title.set_subtitle(&short);
                rv.on_shown();
            }
            None => {
                self.title.set_title("Gitree");
                self.title.set_subtitle("Repositories");
                if let Some(page) = self.tabs.selected_page()
                    && let Some((_, PageKind::Browser(b))) =
                        self.pages.borrow().iter().find(|(p, _)| *p == page)
                    {
                        b.reload();
                    }
            }
        }
    }

    fn current_repo(&self) -> Option<Rc<RepoView>> {
        let page = self.tabs.selected_page()?;
        self.pages.borrow().iter().find_map(|(p, k)| match k {
            PageKind::Repo(r) if *p == page => Some(r.clone()),
            _ => None,
        })
    }

    pub fn new_browser_tab(&self) {
        let w = self.weak();
        let browser = Browser::new(Rc::new(move |p: PathBuf| {
            if let Some(w) = w.upgrade() {
                w.open_repo_replacing_browser(&p);
            }
        }));
        let page = self.tabs.append(&browser.widget);
        page.set_title("Repositories");
        page.set_icon(Some(&gio::ThemedIcon::new("view-list-symbolic")));
        self.pages
            .borrow_mut()
            .push((page.clone(), PageKind::Browser(browser)));
        self.tabs.set_selected_page(&page);
        self.on_page_changed();
    }

    /// Opens `path` (any folder inside a work tree).
    pub fn open_path(&self, path: &Path) {
        match crate::git::find_toplevel(path) {
            Some(top) => {
                config::update(|s| s.add_bookmark(top.clone()));
                self.open_repo_replacing_browser(&top);
            }
            None => super::show_error(
                &self.window,
                "Not a Git repository",
                &path.to_string_lossy(),
            ),
        }
    }

    fn open_repo_replacing_browser(&self, path: &Path) {
        let browser_page = self.tabs.selected_page().filter(|p| {
            self.pages
                .borrow()
                .iter()
                .any(|(pp, k)| pp == p && matches!(k, PageKind::Browser(_)))
        });
        self.open_repo(path);
        if let Some(bp) = browser_page
            && self.tabs.n_pages() > 1 {
                self.tabs.close_page(&bp);
            }
    }

    /// Opens a repository tab (or focuses the existing one).
    pub fn open_repo(&self, path: &Path) {
        let existing = self.pages.borrow().iter().find_map(|(p, k)| match k {
            PageKind::Repo(r) if r.git.workdir == path => Some(p.clone()),
            _ => None,
        });
        if let Some(p) = existing {
            self.tabs.set_selected_page(&p);
            return;
        }
        let w = self.weak();
        let rv = RepoView::new(
            path.to_path_buf(),
            Rc::new(move |p: PathBuf| {
                if let Some(w) = w.upgrade() {
                    w.open_repo(&p);
                }
            }),
        );
        let page = self.tabs.append(&rv.widget);
        page.set_title(&rv.name);
        page.set_tooltip(&path.to_string_lossy());
        page.set_icon(Some(&gio::ThemedIcon::new("gitree-repo-symbolic")));
        self.pages
            .borrow_mut()
            .push((page.clone(), PageKind::Repo(rv)));
        self.tabs.set_selected_page(&page);
        // The first page is auto-selected by append() before it is
        // registered, so make sure the tab gets initialised.
        self.on_page_changed();
        self.save_tabs();
    }

    fn save_tabs(&self) {
        let tabs: Vec<PathBuf> = (0..self.tabs.n_pages())
            .filter_map(|i| {
                let page = self.tabs.nth_page(i);
                self.pages.borrow().iter().find_map(|(p, k)| match k {
                    PageKind::Repo(r) if *p == page => Some(r.git.workdir.clone()),
                    _ => None,
                })
            })
            .collect();
        config::update(|s| s.open_tabs = tabs);
    }
}

fn show_shortcuts(parent: &adw::ApplicationWindow) {
    let rows: &[(&str, &str)] = &[
        ("New tab", "Ctrl+T"),
        ("Close tab", "Ctrl+W"),
        ("Open repository", "Ctrl+O"),
        ("Clone repository", "Ctrl+Shift+N"),
        ("File status / History / Search", "Ctrl+1 / Ctrl+2 / Ctrl+3"),
        ("Commit", "Ctrl+Shift+C"),
        ("Pull", "Ctrl+Shift+L"),
        ("Push", "Ctrl+Shift+P"),
        ("Fetch", "Ctrl+Shift+F"),
        ("New branch", "Ctrl+Shift+B"),
        ("Merge", "Ctrl+Shift+M"),
        ("Stash", "Ctrl+Shift+S"),
        ("Tag", "Ctrl+Shift+T"),
        ("Discard", "Ctrl+Shift+R"),
        ("Refresh", "Ctrl+R / F5"),
        ("Open terminal", "Ctrl+Alt+T"),
        ("Show in Files", "Ctrl+Alt+O"),
        ("Repository settings", "Ctrl+Shift+,"),
        ("Preferences", "Ctrl+,"),
    ];
    let d = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(460)
        .build();
    let group = adw::PreferencesGroup::new();
    for (name, keys) in rows {
        let r = adw::ActionRow::builder().title(*name).build();
        let l = gtk::Label::new(Some(keys));
        l.add_css_class("dim-label");
        r.add_suffix(&l);
        group.add(&r);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    d.set_child(Some(&tv));
    d.set_content_height(560);
    d.present(Some(parent));
}
