//! GTK user interface.

pub mod blame;
pub mod browser;
pub mod dialogs;
pub mod diff_view;
pub mod file_list;
pub mod file_status;
pub mod flow;
pub mod form;
pub mod graph_cell;
pub mod history;
pub mod preferences;
pub mod progress;
pub mod rebase;
pub mod repo_settings;
pub mod repo_view;
pub mod sidebar;
pub mod window;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::future::Future;
use std::path::Path;

/// Spawns a future on the GTK main loop.
pub fn spawn<F: Future<Output = ()> + 'static>(f: F) {
    glib::spawn_future_local(f);
}

/// Runs a blocking closure on a worker thread and awaits its result.
pub async fn bg<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    gio::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn mono_text(text: &str) -> gtk::ScrolledWindow {
    let tv = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    tv.buffer().set_text(text);
    let sw = gtk::ScrolledWindow::builder()
        .child(&tv)
        .min_content_height(80)
        .max_content_height(320)
        .propagate_natural_height(true)
        .build();
    sw.add_css_class("card");
    sw
}

/// Shows an error dialog with (monospace) details.
pub fn show_error(parent: &impl IsA<gtk::Widget>, heading: &str, details: &str) {
    let d = adw::AlertDialog::new(Some(heading), None);
    if !details.trim().is_empty() {
        d.set_extra_child(Some(&mono_text(details.trim())));
    }
    d.add_response("close", "Close");
    d.set_default_response(Some("close"));
    d.set_close_response("close");
    d.present(Some(parent));
}

/// Asks for confirmation; returns true when the user accepts.
pub async fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    ok_label: &str,
    destructive: bool,
) -> bool {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("cancel", "Cancel");
    d.add_response("ok", ok_label);
    d.set_response_appearance(
        "ok",
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    if std::env::var_os("GITREE_AUTO_ACCEPT").is_some() {
        return true;
    }
    d.choose_future(Some(parent)).await == "ok"
}

/// Asks for a single line of text.
pub async fn ask_text(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    initial: &str,
    ok_label: &str,
) -> Option<String> {
    let d = adw::AlertDialog::new(Some(heading), (!body.is_empty()).then_some(body));
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    d.set_extra_child(Some(&entry));
    d.add_response("cancel", "Cancel");
    d.add_response("ok", ok_label);
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    let e2 = entry.clone();
    glib::idle_add_local_once(move || {
        e2.grab_focus();
    });
    let r = d.choose_future(Some(parent)).await;
    let t = entry.text().trim().to_string();
    (r == "ok" && !t.is_empty()).then_some(t)
}

/// Pops up a context menu at (x, y) relative to `widget`.
pub fn popup_menu(widget: &impl IsA<gtk::Widget>, x: f64, y: f64, menu: &gio::Menu) {
    let pop = gtk::PopoverMenu::from_model(Some(menu));
    pop.set_parent(widget.as_ref());
    pop.set_has_arrow(false);
    pop.set_halign(gtk::Align::Start);
    pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    pop.connect_closed(|p| {
        let p = p.clone();
        // Unparent after the activated action has been dispatched.
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

pub fn menu_item_target(menu: &gio::Menu, label: &str, action: &str, target: &str) {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    menu.append_item(&item);
}

pub fn format_time(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(t) => {
            let now = Local::now();
            if t.date_naive() == now.date_naive() {
                format!("Today at {}", t.format("%H:%M"))
            } else {
                t.format("%d %b %Y at %H:%M").to_string()
            }
        }
        _ => String::new(),
    }
}

pub fn format_time_full(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(t) => t.format("%A %d %B %Y %H:%M:%S").to_string(),
        _ => String::new(),
    }
}

pub fn copy_to_clipboard(text: &str) {
    if let Some(d) = gdk::Display::default() {
        d.clipboard().set_text(text);
    }
}

/// Opens a folder in the file manager.
pub fn open_folder(path: &Path) {
    let file = gio::File::for_path(path);
    let _ = gio::AppInfo::launch_default_for_uri(&file.uri(), None::<&gio::AppLaunchContext>);
}

/// Opens a file with its default application.
pub fn open_file(path: &Path) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
}

/// Reveals a file in the file manager.
pub fn show_in_folder(path: &Path) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    launcher.open_containing_folder(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
}

/// Opens a terminal in `dir`, trying the configured terminal first.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    use std::process::Command;
    let custom = crate::config::with(|s| s.terminal.clone());
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    if !custom.trim().is_empty() {
        let mut parts = custom.split_whitespace().map(String::from);
        if let Some(p) = parts.next() {
            candidates.push((p, parts.collect()));
        }
    }
    let d = dir.to_string_lossy().to_string();
    candidates.extend([
        ("ptyxis".into(), vec!["--new-window".into(), "-d".into(), d.clone()]),
        ("kgx".into(), vec!["--working-directory".into(), d.clone()]),
        ("gnome-terminal".into(), vec![format!("--working-directory={d}")]),
        ("konsole".into(), vec!["--workdir".into(), d.clone()]),
        ("xfce4-terminal".into(), vec![format!("--working-directory={d}")]),
        ("x-terminal-emulator".into(), vec![]),
        ("xterm".into(), vec![]),
    ]);
    for (prog, args) in candidates {
        if Command::new(&prog).args(&args).current_dir(dir).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("No terminal emulator found. Set one in Preferences.".into())
}

/// Letter shown for a file status plus its CSS class suffix.
pub fn status_badge(code: char) -> (String, &'static str) {
    match code {
        'A' => ("+".into(), "A"),
        'M' => ("M".into(), "M"),
        'D' => ("−".into(), "D"),
        'R' => ("R".into(), "R"),
        'C' => ("C".into(), "C"),
        'T' => ("T".into(), "M"),
        'U' => ("!".into(), "U"),
        '?' => ("?".into(), "Q"),
        '!' => ("I".into(), "I"),
        c => (c.to_string(), "M"),
    }
}

pub fn status_tooltip(code: char) -> &'static str {
    match code {
        'A' => "Added",
        'M' => "Modified",
        'D' => "Deleted",
        'R' => "Renamed",
        'C' => "Copied",
        'T' => "Type changed",
        'U' => "Conflicted",
        '?' => "Untracked",
        '!' => "Ignored",
        _ => "",
    }
}

/// A small pill label showing a count.
pub fn count_badge() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.add_css_class("count-badge");
    l.set_valign(gtk::Align::Center);
    l.set_visible(false);
    l
}

pub fn set_badge(l: &gtk::Label, n: usize) {
    l.set_text(&n.to_string());
    l.set_visible(n > 0);
}

/// Toast helper: finds the nearest ToastOverlay ancestor.
pub fn toast(widget: &impl IsA<gtk::Widget>, msg: &str) {
    if let Some(o) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        o.add_toast(adw::Toast::new(msg));
    }
}

/// TextViews don't size themselves to their content; give a monospace
/// view room for `chars` characters.
pub fn set_mono_width(tv: &gtk::TextView, chars: usize) {
    let layout = tv.create_pango_layout(Some(&"0".repeat(chars.max(1))));
    layout.set_font_description(Some(&gtk::pango::FontDescription::from_string("Monospace")));
    let (w, _) = layout.pixel_size();
    tv.set_size_request(w + tv.left_margin() + tv.right_margin() + 4, -1);
}

pub fn is_dark() -> bool {
    adw::StyleManager::default().is_dark()
}
