//! Runs git commands with a streaming progress sheet: output is
//! streamed live, the operation can be cancelled, and failures keep the
//! sheet open with the full output.

use crate::git::{describe, Git, GitError, Prompt};
use crate::i18n::{gettext, gettext_f};
use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::process::Child;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct OpOptions {
    /// Show the progress sheet immediately (network operations).
    pub network: bool,
    /// Extra environment variables for every command.
    pub env: Vec<(String, String)>,
    /// Never show UI; return the error to the caller.
    pub quiet: bool,
}

enum Msg {
    Cmd(String),
    Line(String),
    Done(Result<String, GitError>),
}

struct Sheet {
    window: adw::Window,
    status: gtk::Label,
    spinner: adw::Spinner,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    button: gtk::Button,
    copy: gtk::Button,
    error: gtk::Label,
    error_box: gtk::Box,
}

/// A resizable modal window: status line, error summary, and the command
/// output filling the rest (the only scrolling area).
fn build_sheet(parent: &gtk::Widget, title: &str) -> Sheet {
    let (w, h) = crate::config::with(|s| (s.progress_width, s.progress_height));
    let window = adw::Window::builder()
        .title(title)
        .modal(true)
        .resizable(true)
        .deletable(false)
        .default_width(w.max(420))
        .default_height(h.max(260))
        .build();
    window.set_size_request(420, 260);
    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        window.set_transient_for(Some(&root));
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(6);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let spinner = adw::Spinner::new();
    spinner.set_size_request(20, 20);
    let status = gtk::Label::builder()
        .label(gettext("Starting…"))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .hexpand(true)
        .build();
    status.add_css_class("heading");
    row.append(&spinner);
    row.append(&status);
    vbox.append(&row);

    // Error summary: a few key lines at their natural height.
    let error_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    error_box.add_css_class("error-banner");
    error_box.add_css_class("card");
    let icon = gtk::Image::from_icon_name("dialog-error-symbolic");
    icon.set_valign(gtk::Align::Start);
    icon.add_css_class("error");
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .hexpand(true)
        .build();
    error_box.append(&icon);
    error_box.append(&error);
    error_box.set_visible(false);
    vbox.append(&error_box);

    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(8)
        .right_margin(8)
        .top_margin(6)
        .bottom_margin(6)
        .build();
    let sw = gtk::ScrolledWindow::builder()
        .child(&view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(80)
        .build();
    sw.add_css_class("card");
    vbox.append(&sw);

    // Buttons live in a bottom bar so they always stay visible.
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_margin_top(6);
    actions.set_margin_bottom(12);
    actions.set_margin_start(12);
    actions.set_margin_end(12);
    let copy = gtk::Button::with_label(&gettext("Copy Output"));
    copy.set_visible(false);
    let b2 = buffer.clone();
    copy.connect_clicked(move |b| {
        super::copy_to_clipboard(&b2.text(&b2.start_iter(), &b2.end_iter(), false));
        b.set_label(&gettext("Copied"));
    });
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let button = gtk::Button::with_label(&gettext("Cancel"));
    button.add_css_class("pill");
    actions.append(&copy);
    actions.append(&spacer);
    actions.append(&button);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&vbox));
    tv.add_bottom_bar(&actions);
    window.set_content(Some(&tv));

    // Remember the size the user gives the window.
    window.connect_close_request(|w| {
        if !w.is_maximized() {
            let (w, h) = (w.width(), w.height());
            if w > 0 && h > 0 {
                crate::config::update(|s| {
                    s.progress_width = w;
                    s.progress_height = h;
                });
            }
        }
        glib::Propagation::Proceed
    });
    // Escape closes once the operation has finished.
    let keys = gtk::EventControllerKey::new();
    let w2 = window.clone();
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape && w2.is_deletable() {
            w2.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);

    Sheet {
        window,
        status,
        spinner,
        buffer,
        view,
        button,
        copy,
        error,
        error_box,
    }
}

/// "Writing objects" for "Writing objects:  42% (3/7)"; None for normal lines.
fn progress_prefix(line: &str) -> Option<String> {
    line.match_indices(':').find_map(|(i, _)| {
        let token = line[i + 1..].split_whitespace().next()?;
        let pct = token.strip_suffix('%')?;
        (!pct.is_empty() && pct.chars().all(|c| c.is_ascii_digit())).then(|| line[..i].to_string())
    })
}

/// The few lines of git output that explain a failure (full output is
/// stays in the output log below).
fn error_summary(e: &GitError) -> String {
    let text = format!("{}\n{}", e.stderr, e.stdout);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && progress_prefix(l).is_none())
        .collect();
    let key: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| {
            let lower = l.to_lowercase();
            lower.starts_with("error")
                || lower.starts_with("fatal")
                || lower.starts_with("! ")
                || lower.contains("rejected")
                || lower.contains("conflict")
                || lower.contains("cancelled")
        })
        .collect();
    let chosen: Vec<&str> = if key.is_empty() {
        lines[lines.len().saturating_sub(4)..].to_vec()
    } else {
        key.into_iter().take(8).collect()
    };
    let summary: Vec<String> = chosen
        .iter()
        .map(|l| {
            if l.chars().count() > 300 {
                format!("{}…", l.chars().take(300).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();
    if summary.is_empty() {
        match e.code {
            Some(c) => gettext_f("{command} failed (exit code {code}).", &[("command", &e.command), ("code", &c.to_string())]),
            None => gettext_f("{command} was interrupted.", &[("command", &e.command)]),
        }
    } else {
        summary.join("\n")
    }
}

/// Runs `cmds` sequentially, stopping at the first failure.
/// Resolves to the stdout of the last command.
pub async fn run(
    parent: &gtk::Widget,
    git: &Git,
    title: &str,
    cmds: Vec<Vec<String>>,
    opts: OpOptions,
) -> Result<String, GitError> {
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let handle: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let cancelled = Arc::new(AtomicBool::new(false));

    {
        let git = git.clone();
        let handle = handle.clone();
        let cancelled = cancelled.clone();
        let env = opts.env.clone();
        let prompt = if opts.quiet {
            Prompt::Never
        } else {
            Prompt::Interactive
        };
        std::thread::spawn(move || {
            let mut last: Result<String, GitError> = Ok(String::new());
            for c in cmds {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tx.send_blocking(Msg::Cmd(describe(&c)));
                let tx2 = tx.clone();
                last = git.run_streaming(&c, prompt, &env, handle.clone(), move |l| {
                    let _ = tx2.send_blocking(Msg::Line(l));
                });
                if last.is_err() {
                    break;
                }
            }
            let _ = tx.send_blocking(Msg::Done(last));
        });
    }

    if opts.quiet {
        loop {
            match rx.recv().await {
                Ok(Msg::Done(r)) => return r,
                Ok(_) => continue,
                Err(_) => {
                    return Err(GitError {
                        command: title.into(),
                        stderr: gettext("Worker ended unexpectedly"),
                        stdout: String::new(),
                        code: None,
                    })
                }
            }
        }
    }

    let sheet = Rc::new(build_sheet(parent, title));
    let presented = Rc::new(Cell::new(false));
    let finished = Rc::new(Cell::new(false));

    {
        let handle = handle.clone();
        let cancelled = cancelled.clone();
        let finished = finished.clone();
        let window = sheet.window.clone();
        sheet.button.connect_clicked(move |b| {
            if finished.get() {
                window.close();
            } else {
                cancelled.store(true, Ordering::SeqCst);
                if let Some(c) = handle.lock().unwrap().as_mut() {
                    let _ = c.kill();
                }
                b.set_sensitive(false);
            }
        });
    }

    let delay = if opts.network { 100 } else { 600 };
    {
        let sheet = sheet.clone();
        let presented = presented.clone();
        let finished = finished.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if !finished.get() && !presented.get() {
                presented.set(true);
                sheet.window.present();
            }
        });
    }

    let mut last_progress: Option<String> = None;
    let mut line_start: Option<gtk::TextMark> = None;
    let result = loop {
        match rx.recv().await {
            Ok(Msg::Cmd(c)) => {
                last_progress = None;
                sheet.status.set_text(&c);
                let mut end = sheet.buffer.end_iter();
                sheet.buffer.insert(&mut end, &format!("$ {c}\n"));
            }
            Ok(Msg::Line(l)) => {
                // Git redraws progress ("Writing objects: 42% ...") with \r:
                // replace the previous line instead of appending a new one.
                let prefix = progress_prefix(&l);
                if prefix.is_some() && prefix == last_progress {
                    if let Some(mark) = &line_start {
                        let mut start = sheet.buffer.iter_at_mark(mark);
                        let mut end = sheet.buffer.end_iter();
                        sheet.buffer.delete(&mut start, &mut end);
                    }
                } else {
                    let end = sheet.buffer.end_iter();
                    match &line_start {
                        Some(m) => sheet.buffer.move_mark(m, &end),
                        None => line_start = Some(sheet.buffer.create_mark(None, &end, true)),
                    }
                }
                last_progress = prefix;
                let mut end = sheet.buffer.end_iter();
                sheet.buffer.insert(&mut end, &format!("{l}\n"));
                sheet.status.set_text(&l);
                let mark = sheet.buffer.create_mark(None, &sheet.buffer.end_iter(), false);
                sheet.view.scroll_mark_onscreen(&mark);
                sheet.buffer.delete_mark(&mark);
            }
            Ok(Msg::Done(r)) => break r,
            Err(_) => {
                break Err(GitError {
                    command: title.into(),
                    stderr: gettext("Worker ended unexpectedly"),
                    stdout: String::new(),
                    code: None,
                })
            }
        }
    };
    finished.set(true);

    match &result {
        Ok(_) => {
            if presented.get() {
                sheet.window.set_deletable(true);
                sheet.window.close();
            } else {
                sheet.window.destroy();
            }
        }
        Err(e) => {
            sheet.spinner.set_visible(false);
            let was_cancelled = cancelled.load(Ordering::SeqCst);
            sheet.status.set_text(&if was_cancelled {
                gettext("Cancelled")
            } else {
                gettext("Completed with errors, see below.")
            });
            if !was_cancelled {
                let msg = error_summary(e);
                sheet.error.set_text(&msg);
                sheet.error_box.set_visible(true);
                sheet.copy.set_visible(true);
                let (view, buffer) = (sheet.view.clone(), sheet.buffer.clone());
                glib::idle_add_local_once(move || {
                    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
                    view.scroll_to_mark(&mark, 0.0, false, 0.0, 1.0);
                    buffer.delete_mark(&mark);
                });
            }
            sheet.button.set_label(&gettext("Close"));
            sheet.button.set_sensitive(true);
            sheet.window.set_deletable(true);
            let (ctx, crx) = async_channel::bounded::<()>(1);
            sheet.window.connect_destroy(move |_| {
                let _ = ctx.try_send(());
            });
            if was_cancelled {
                sheet.window.close();
            } else if !presented.get() {
                presented.set(true);
                sheet.window.present();
            }
            let _ = crx.recv().await;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(stderr: &str) -> GitError {
        GitError {
            command: "git push".into(),
            stderr: stderr.into(),
            stdout: String::new(),
            code: Some(1),
        }
    }

    #[test]
    fn summary_keeps_key_lines_only() {
        let mut long = String::new();
        for i in 0..200 {
            long.push_str(&format!("remote: hook line {i}\n"));
        }
        long.push_str(" ! [remote rejected] main -> main (pre-receive hook declined)\nerror: failed to push some refs to 'origin'\n");
        let s = error_summary(&err(&long));
        assert_eq!(s.lines().count(), 2);
        assert!(s.contains("failed to push"));
    }

    #[test]
    fn progress_prefixes() {
        assert_eq!(progress_prefix("Writing objects:  42% (3/7)").as_deref(), Some("Writing objects"));
        assert_eq!(progress_prefix("remote: Counting objects: 100% (5/5), done.").as_deref(), Some("remote: Counting objects"));
        assert_eq!(progress_prefix("remote: hook says hi"), None);
        assert_eq!(progress_prefix("To ../origin.git"), None);
    }

    #[test]
    fn summary_falls_back_to_last_lines() {
        let s = error_summary(&err("a\nb\nc\nd\ne\nf\n"));
        assert_eq!(s, "c\nd\ne\nf");
    }
}
