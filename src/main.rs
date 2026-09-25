mod askpass;
mod config;
mod git;
mod host;
mod i18n;
mod ui;
mod watch;

use adw::prelude::*;
use gtk::{gio, glib};

/// Environment variable marking that we were launched by git as the
/// GIT_ASKPASS / SSH_ASKPASS helper.
pub const ASKPASS_ENV: &str = "GITREE_ASKPASS";
pub const APP_ID: &str = "io.github.gitree.Gitree";

fn main() -> glib::ExitCode {
    i18n::init();
    if std::env::var_os(ASKPASS_ENV).is_some() {
        return askpass::run();
    }

    gio::resources_register_include!("gitree.gresource").expect("failed to register resources");

    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    if std::env::var_os("GITREE_SCREENSHOT").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .resource_base_path("/io/github/gitree/Gitree")
        .build();

    app.connect_startup(|app| {
        gtk::Window::set_default_icon_name(APP_ID);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/io/github/gitree/Gitree/icons");
        }
        ui::window::setup_app(app);
    });
    app.connect_activate(|app| {
        ui::window::MainWindow::get_or_create(app).present();
    });
    app.connect_open(|app, files, _| {
        let w = ui::window::MainWindow::get_or_create(app);
        for f in files {
            if let Some(p) = f.path() {
                w.open_path(&p);
            }
        }
        w.present();
    });
    app.run()
}
