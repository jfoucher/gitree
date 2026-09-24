//! Blame / annotate window.

use super::repo_view::RepoView;
use super::{bg, spawn};
use crate::git::blame;
use adw::prelude::*;
use gtk::gdk;
use sourceview5::prelude::*;
use std::rc::Rc;

pub fn show(rv: &Rc<RepoView>, path: &str, rev: Option<String>) {
    let title = match &rev {
        Some(r) => format!("Blame — {path} @ {}", &r[..r.len().min(10)]),
        None => format!("Blame — {path}"),
    };
    let win = adw::Window::builder()
        .title(&title)
        .default_width(1100)
        .default_height(760)
        .build();
    if let Some(root) = rv.widget.root().and_downcast::<gtk::Window>() {
        win.set_transient_for(Some(&root));
    }
    let header = adw::HeaderBar::new();
    let spinner = adw::Spinner::new();
    header.pack_start(&spinner);
    let info = gtk::Label::new(Some("Click a line's annotation to show the commit in History"));
    info.add_css_class("dim-label");
    info.add_css_class("caption");
    header.pack_end(&info);

    let gutter_buf = gtk::TextBuffer::new(None);
    let gutter = gtk::TextView::builder()
        .buffer(&gutter_buf)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .left_margin(6)
        .right_margin(6)
        .build();
    gutter.add_css_class("blame-gutter");
    gutter.add_css_class("diff-gutter");

    let buf = sourceview5::Buffer::new(None);
    if let Some(l) = sourceview5::LanguageManager::default().guess_language(Some(path), None::<&str>) {
        buf.set_language(Some(&l));
    }
    let scheme = if super::is_dark() { "Adwaita-dark" } else { "Adwaita" };
    if let Some(s) = sourceview5::StyleSchemeManager::default().scheme(scheme) {
        buf.set_style_scheme(Some(&s));
    }
    let view = sourceview5::View::builder()
        .buffer(&buf)
        .editable(false)
        .monospace(true)
        .show_line_numbers(true)
        .hexpand(true)
        .left_margin(6)
        .build();
    view.add_css_class("diff-text");

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hbox.append(&gutter);
    hbox.append(&view);
    let sw = gtk::ScrolledWindow::builder().child(&hbox).vexpand(true).build();

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&sw));
    win.set_content(Some(&tv));
    win.present();

    let git = rv.git.clone();
    let p = path.to_string();
    let rv_weak = rv.weak();
    let win2 = win.clone();
    spawn(async move {
        let r = bg(move || blame::blame(&git, &p, rev.as_deref())).await;
        spinner.set_visible(false);
        let b = match r {
            Ok(b) => b,
            Err(e) => {
                super::show_error(&win2, "Blame failed", &e.to_string());
                return;
            }
        };
        let mut text = String::new();
        let mut gtext = String::new();
        let mut line_oids = Vec::with_capacity(b.lines.len());
        let mut prev = String::new();
        let mut block_starts = Vec::new();
        for (i, l) in b.lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
                gtext.push('\n');
            }
            text.push_str(&l.text);
            if l.oid != prev {
                let c = b.commits.get(&l.oid);
                let date = c
                    .map(|c| {
                        use chrono::TimeZone;
                        chrono::Local
                            .timestamp_opt(c.author_time, 0)
                            .single()
                            .map(|t| t.format("%Y-%m-%d").to_string())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let author: String = c.map(|c| c.author.chars().take(16).collect()).unwrap_or_default();
                if l.oid.starts_with("0000000") {
                    gtext.push_str(&format!("{:<8} {:<16} {:<10}", "-------", "Not committed", ""));
                } else {
                    gtext.push_str(&format!("{:<8} {:<16} {:<10}", &l.oid[..7], author, date));
                }
                block_starts.push(i);
                prev = l.oid.clone();
            } else {
                gtext.push_str(&" ".repeat(36));
            }
            line_oids.push(l.oid.clone());
        }
        buf.set_text(&text);
        gutter_buf.set_text(&gtext);
        super::set_mono_width(&gutter, 36);

        // Alternate block backgrounds to make commit boundaries visible.
        let shade = gtk::TextTag::builder()
            .name("shade")
            .paragraph_background_rgba(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.08))
            .build();
        buf.tag_table().add(&shade);
        let gshade = gtk::TextTag::builder()
            .name("shade")
            .paragraph_background_rgba(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.08))
            .build();
        gutter_buf.tag_table().add(&gshade);
        for (bi, &start) in block_starts.iter().enumerate() {
            if bi % 2 == 1 {
                let end = block_starts.get(bi + 1).copied().unwrap_or(line_oids.len());
                for (bb, name) in [(buf.upcast_ref::<gtk::TextBuffer>(), "shade"), (&gutter_buf, "shade")] {
                    if let (Some(s), Some(e)) = (bb.iter_at_line(start as i32), bb.iter_at_line(end as i32).or(Some(bb.end_iter()))) {
                        bb.apply_tag_by_name(name, &s, &e);
                    }
                }
            }
        }

        // Tooltips with commit summaries & click to jump.
        let commits = Rc::new(b.commits);
        let oids = Rc::new(line_oids);
        gutter.set_has_tooltip(true);
        let (c2, o2) = (commits.clone(), oids.clone());
        gutter.connect_query_tooltip(move |g, x, y, _, tip| {
            let (bx, by) = g.window_to_buffer_coords(gtk::TextWindowType::Widget, x, y);
            let Some(it) = g.iter_at_location(bx, by) else { return false };
            let Some(oid) = o2.get(it.line() as usize) else { return false };
            let Some(c) = c2.get(oid) else { return false };
            tip.set_text(Some(&format!("{}\n{}\n{}", &oid[..10.min(oid.len())], c.author, c.summary)));
            true
        });
        let click = gtk::GestureClick::new();
        let g2 = gutter.clone();
        click.connect_released(move |_, _, x, y| {
            let (bx, by) = g2.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let Some(it) = g2.iter_at_location(bx, by) else { return };
            let Some(oid) = oids.get(it.line() as usize).cloned() else { return };
            if oid.starts_with("0000000") {
                return;
            }
            if let Some(rv) = rv_weak.upgrade() {
                rv.jump_to_commit(&oid);
                if let Some(w) = rv.widget.root().and_downcast::<gtk::Window>() {
                    w.present();
                }
            }
        });
        gutter.add_controller(click);
    });
}
