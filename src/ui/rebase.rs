//! Interactive rebase dialog: reorder (drag & drop or buttons), squash,
//! reword, edit and drop commits.

use super::progress::OpOptions;
use super::repo_view::RepoView;
use super::{bg, show_error};
use crate::git::log;
use crate::git::rebase::{self, RebaseAction, RebaseItem};
use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct State {
    items: RefCell<Vec<RebaseItem>>,
    original: Vec<RebaseItem>,
    list: gtk::ListBox,
    selected: Cell<Option<usize>>,
}

pub async fn show(rv: &Rc<RepoView>, base: Option<String>) {
    if !rv.snapshot().op.is_none() {
        show_error(&rv.widget, "Operation in progress", "Finish or abort the current operation first.");
        return;
    }
    let git = rv.git.clone();
    let b2 = base.clone();
    let commits = match bg(move || log::range_oldest_first(&git, b2.as_deref())).await {
        Ok(c) => c,
        Err(e) => {
            show_error(&rv.widget, "Could not list commits", &e.to_string());
            return;
        }
    };
    if commits.is_empty() {
        rv.toast("There are no commits to rebase after that commit");
        return;
    }
    if commits.iter().any(|c| c.parents.len() > 1) {
        show_error(
            &rv.widget,
            "Merge commits in range",
            "The selected range contains merge commits, which interactive rebase would flatten. Pick a later base commit.",
        );
        return;
    }
    let items: Vec<RebaseItem> = commits
        .iter()
        .map(|c| RebaseItem {
            oid: c.oid.clone(),
            subject: c.subject.clone(),
            action: RebaseAction::Pick,
            new_message: None,
        })
        .collect();

    let dialog = adw::Dialog::builder()
        .title("Interactive Rebase")
        .content_width(760)
        .content_height(560)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("Start Rebase");
    ok.add_css_class("suggested-action");
    header.pack_start(&cancel);
    header.pack_end(&ok);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    let hint = gtk::Label::builder()
        .label(format!(
            "Commits are applied from top to bottom on top of {}. Drag rows to reorder.",
            base.as_deref().map(|b| &b[..b.len().min(12)]).unwrap_or("the root")
        ))
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("dim-label");
    vbox.append(&hint);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    let sw = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    vbox.append(&sw);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let up = gtk::Button::from_icon_name("go-up-symbolic");
    up.set_tooltip_text(Some("Move up"));
    let down = gtk::Button::from_icon_name("go-down-symbolic");
    down.set_tooltip_text(Some("Move down"));
    let squash = gtk::Button::with_label("Squash with Previous");
    let edit_msg = gtk::Button::with_label("Edit Message…");
    let delete = gtk::Button::with_label("Delete");
    delete.add_css_class("destructive-action");
    let reset = gtk::Button::with_label("Reset");
    for b in [&up, &down, &squash, &edit_msg, &delete] {
        buttons.append(b);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.append(&spacer);
    buttons.append(&reset);
    vbox.append(&buttons);

    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&vbox));
    dialog.set_child(Some(&tv));

    let state = Rc::new(State {
        items: RefCell::new(items.clone()),
        original: items,
        list: list.clone(),
        selected: Cell::new(None),
    });
    rebuild(&state);

    let st = state.clone();
    list.connect_row_selected(move |_, row| {
        st.selected.set(row.map(|r| r.index() as usize));
    });

    let st = state.clone();
    up.connect_clicked(move |_| {
        if let Some(i) = st.selected.get().filter(|&i| i > 0) {
            st.items.borrow_mut().swap(i, i - 1);
            st.selected.set(Some(i - 1));
            rebuild(&st);
        }
    });
    let st = state.clone();
    down.connect_clicked(move |_| {
        let n = st.items.borrow().len();
        if let Some(i) = st.selected.get().filter(|&i| i + 1 < n) {
            st.items.borrow_mut().swap(i, i + 1);
            st.selected.set(Some(i + 1));
            rebuild(&st);
        }
    });
    let st = state.clone();
    squash.connect_clicked(move |_| {
        if let Some(i) = st.selected.get().filter(|&i| i > 0) {
            st.items.borrow_mut()[i].action = RebaseAction::Squash;
            rebuild(&st);
        }
    });
    let st = state.clone();
    delete.connect_clicked(move |_| {
        if let Some(i) = st.selected.get() {
            st.items.borrow_mut()[i].action = RebaseAction::Drop;
            rebuild(&st);
        }
    });
    let st = state.clone();
    reset.connect_clicked(move |_| {
        *st.items.borrow_mut() = st.original.clone();
        rebuild(&st);
    });
    let st = state.clone();
    let git = rv.git.clone();
    let dlg = dialog.clone();
    edit_msg.connect_clicked(move |_| {
        let Some(i) = st.selected.get() else { return };
        let st = st.clone();
        let git = git.clone();
        let dlg = dlg.clone();
        glib::spawn_future_local(async move {
            let (oid, current) = {
                let it = &st.items.borrow()[i];
                (it.oid.clone(), it.new_message.clone())
            };
            let initial = match current {
                Some(m) => m,
                None => log::message(&git, &oid).unwrap_or_default().trim().to_string(),
            };
            let d = adw::AlertDialog::new(Some("Edit Commit Message"), None);
            let tv = gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .top_margin(6)
                .bottom_margin(6)
                .left_margin(6)
                .right_margin(6)
                .monospace(true)
                .build();
            tv.buffer().set_text(&initial);
            let sw = gtk::ScrolledWindow::builder()
                .child(&tv)
                .min_content_height(160)
                .min_content_width(420)
                .build();
            sw.add_css_class("card");
            d.set_extra_child(Some(&sw));
            d.add_responses(&[("cancel", "Cancel"), ("ok", "OK")]);
            d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
            let tv2 = tv.clone();
            glib::idle_add_local_once(move || {
                tv2.grab_focus();
            });
            if d.choose_future(Some(&dlg)).await == "ok" {
                let m = super::form::text_of(&tv);
                if !m.trim().is_empty() {
                    let mut items = st.items.borrow_mut();
                    items[i].new_message = Some(m.trim().to_string());
                    if items[i].action == RebaseAction::Pick {
                        items[i].action = RebaseAction::Reword;
                    }
                }
                rebuild(&st);
            }
        });
    });

    let (tx, rx) = async_channel::bounded::<bool>(1);
    let (t1, d1) = (tx.clone(), dialog.clone());
    ok.connect_clicked(move |_| {
        let _ = t1.try_send(true);
        d1.close();
    });
    let d2 = dialog.clone();
    cancel.connect_clicked(move |_| {
        d2.close();
    });
    dialog.connect_closed(move |_| {
        let _ = tx.try_send(false);
    });
    dialog.present(Some(&rv.widget));
    if !rx.recv().await.unwrap_or(false) {
        return;
    }

    let items = state.items.borrow().clone();
    if let Some(first) = items.iter().find(|i| i.action != RebaseAction::Drop)
        && matches!(first.action, RebaseAction::Squash | RebaseAction::Fixup) {
            show_error(&rv.widget, "Invalid rebase", "The first commit can't be squashed: there is nothing before it to squash into.");
            return;
        }
    if items.iter().all(|i| i.action == RebaseAction::Drop)
        && !super::confirm(&rv.widget, "Drop All Commits?", "Every commit in the range will be removed.", "Continue", true).await
    {
        return;
    }
    let dir = rv.git_dir.join("gitree-rebase");
    let _ = std::fs::remove_dir_all(&dir);
    let todo = match rebase::write_todo(&items, &dir) {
        Ok(t) => t,
        Err(e) => {
            show_error(&rv.widget, "Could not prepare rebase", &e.to_string());
            return;
        }
    };
    let mut cmd = vec!["rebase".to_string(), "-i".to_string()];
    match &base {
        Some(b) => cmd.push(b.clone()),
        None => cmd.push("--root".into()),
    }
    rv.run_ops(
        "Interactive Rebase",
        vec![cmd],
        OpOptions {
            env: vec![("GIT_SEQUENCE_EDITOR".into(), rebase::sequence_editor(&todo))],
            ..Default::default()
        },
    )
    .await;
}

fn rebuild(state: &Rc<State>) {
    while let Some(c) = state.list.first_child() {
        state.list.remove(&c);
    }
    let items = state.items.borrow().clone();
    for (i, it) in items.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(
                it.new_message
                    .as_deref()
                    .and_then(|m| m.lines().next())
                    .unwrap_or(&it.subject),
            ))
            .subtitle(&it.oid[..it.oid.len().min(10)])
            .build();
        if it.action == RebaseAction::Drop {
            row.add_css_class("dim-label");
        }
        let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        row.add_prefix(&handle);
        let names: Vec<&str> = RebaseAction::ALL.iter().map(|a| a.label()).collect();
        let dd = gtk::DropDown::from_strings(&names);
        dd.set_valign(gtk::Align::Center);
        dd.set_selected(RebaseAction::ALL.iter().position(|a| *a == it.action).unwrap_or(0) as u32);
        let st = state.clone();
        dd.connect_selected_notify(move |d| {
            let a = RebaseAction::ALL[d.selected() as usize];
            let mut items = st.items.borrow_mut();
            if items[i].action != a {
                items[i].action = a;
                drop(items);
                let st2 = st.clone();
                glib::idle_add_local_once(move || rebuild(&st2));
            }
        });
        row.add_suffix(&dd);

        // Drag & drop reordering.
        let src = gtk::DragSource::new();
        src.set_actions(gdk::DragAction::MOVE);
        src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&(i as u32).to_value())));
        row.add_controller(src);
        let target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        let st = state.clone();
        target.connect_drop(move |_, v, _, _| {
            let Ok(from) = v.get::<u32>() else { return false };
            let from = from as usize;
            if from == i {
                return false;
            }
            {
                let mut items = st.items.borrow_mut();
                let it = items.remove(from);
                items.insert(i, it);
            }
            st.selected.set(Some(i));
            let st2 = st.clone();
            glib::idle_add_local_once(move || rebuild(&st2));
            true
        });
        row.add_controller(target);
        state.list.append(&row);
    }
    if let Some(sel) = state.selected.get()
        && let Some(r) = state.list.row_at_index(sel as i32) {
            state.list.select_row(Some(&r));
        }
}
