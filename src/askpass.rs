//! Credential prompt shown when git (or ssh) needs a username, password or
//! passphrase. Git runs our own executable as GIT_ASKPASS/SSH_ASKPASS with
//! the prompt as the first argument and reads the answer from stdout.

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub fn run() -> glib::ExitCode {
    let prompt = std::env::args().nth(1).unwrap_or_else(|| "Password:".into());
    let lower = prompt.to_lowercase();
    // ssh host key confirmation prompts expect "yes"/"no".
    let is_confirm = lower.contains("(yes/no");
    let is_secret = !is_confirm && !lower.starts_with("username");

    let app = adw::Application::builder()
        .application_id("io.github.gitree.Gitree.Askpass")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let answer: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let a2 = answer.clone();
    app.connect_activate(move |app| {
        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title("Authentication Required")
            .default_width(460)
            .resizable(false)
            .build();
        let header = adw::HeaderBar::new();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_top(12);
        body.set_margin_bottom(18);
        body.set_margin_start(18);
        body.set_margin_end(18);

        let title = gtk::Label::builder()
            .label("Git needs your credentials")
            .xalign(0.0)
            .build();
        title.add_css_class("title-3");
        let label = gtk::Label::builder()
            .label(&prompt)
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build();
        body.append(&title);
        body.append(&label);

        let group = adw::PreferencesGroup::new();
        let entry: gtk::Widget = if is_confirm {
            adw::EntryRow::builder().title("Type yes or no").text("yes").build().upcast()
        } else if is_secret {
            adw::PasswordEntryRow::builder().title("Password").build().upcast()
        } else {
            adw::EntryRow::builder().title("Username").build().upcast()
        };
        group.add(&entry);
        body.append(&group);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("suggested-action");
        buttons.append(&cancel);
        buttons.append(&ok);
        body.append(&buttons);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&body));
        win.set_content(Some(&tv));

        let submit = {
            let a = a2.clone();
            let win = win.clone();
            let entry = entry.clone();
            move || {
                let text = entry
                    .downcast_ref::<gtk::Editable>()
                    .map(|e| e.text().to_string())
                    .unwrap_or_default();
                *a.borrow_mut() = Some(text);
                win.close();
            }
        };
        let s1 = submit.clone();
        ok.connect_clicked(move |_| s1());
        if let Some(e) = entry.downcast_ref::<adw::EntryRow>() {
            let s = submit.clone();
            e.connect_entry_activated(move |_| s());
        }
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
        win.present();
        entry.grab_focus();
    });
    app.run_with_args::<&str>(&[]);

    match answer.borrow().as_ref() {
        Some(a) => {
            println!("{a}");
            glib::ExitCode::SUCCESS
        }
        None => glib::ExitCode::FAILURE,
    }
}
