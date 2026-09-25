//! Small builder for option dialogs (Pull, Push, Branch, ...), mimicking
//! macOS-style sheets with libadwaita preference rows.

use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Form {
    pub dialog: adw::Dialog,
    page: gtk::Box,
    group: RefCell<adw::PreferencesGroup>,
    pub ok: gtk::Button,
    cancel: gtk::Button,
    focus: RefCell<Option<gtk::Widget>>,
    tx: async_channel::Sender<bool>,
    rx: async_channel::Receiver<bool>,
    validators: RefCell<Vec<Rc<dyn Fn() -> bool>>>,
}

impl Form {
    pub fn new(title: &str, ok_label: &str) -> Rc<Self> {
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(520)
            .build();
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label(ok_label);
        ok.add_css_class("suggested-action");
        header.pack_start(&cancel);
        header.pack_end(&ok);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.set_margin_top(12);
        page.set_margin_bottom(18);
        page.set_margin_start(18);
        page.set_margin_end(18);
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&page)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(640)
            .build();
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&scrolled));
        dialog.set_child(Some(&tv));
        dialog.set_default_widget(Some(&ok));

        let group = adw::PreferencesGroup::new();
        page.append(&group);

        let (tx, rx) = async_channel::bounded(1);
        let form = Rc::new(Self {
            dialog,
            page,
            group: RefCell::new(group),
            ok: ok.clone(),
            cancel: cancel.clone(),
            focus: RefCell::new(None),
            tx,
            rx,
            validators: RefCell::new(Vec::new()),
        });

        let f = Rc::downgrade(&form);
        ok.connect_clicked(move |_| {
            if let Some(f) = f.upgrade() {
                let _ = f.tx.try_send(true);
                f.dialog.close();
            }
        });
        let d = form.dialog.clone();
        cancel.connect_clicked(move |_| {
            d.close();
        });
        let tx = form.tx.clone();
        form.dialog.connect_closed(move |_| {
            let _ = tx.try_send(false);
        });
        form
    }

    /// Starts a new titled group of rows.
    pub fn group(&self, title: &str) -> adw::PreferencesGroup {
        let g = adw::PreferencesGroup::new();
        if !title.is_empty() {
            g.set_title(title);
        }
        self.page.append(&g);
        *self.group.borrow_mut() = g.clone();
        g
    }

    pub fn description(&self, text: &str) {
        self.group.borrow().set_description(Some(text));
    }

    pub fn add(&self, w: &impl IsA<gtk::Widget>) {
        self.group.borrow().add(w);
    }

    pub fn entry(&self, title: &str, initial: &str) -> adw::EntryRow {
        let e = adw::EntryRow::builder()
            .title(title)
            .text(initial)
            .activates_default(true)
            .build();
        self.add(&e);
        e
    }

    pub fn combo(&self, title: &str, items: &[String], selected: Option<&str>) -> adw::ComboRow {
        let model = gtk::StringList::new(&items.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let c = adw::ComboRow::builder().title(title).model(&model).build();
        if items.len() > 12 {
            c.set_enable_search(true);
            c.set_expression(Some(gtk::PropertyExpression::new(
                gtk::StringObject::static_type(),
                None::<&gtk::Expression>,
                "string",
            )));
        }
        if let Some(sel) = selected
            && let Some(i) = items.iter().position(|s| s == sel) {
                c.set_selected(i as u32);
            }
        self.add(&c);
        c
    }

    pub fn switch(&self, title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
        let s = adw::SwitchRow::builder().title(title).active(active).build();
        if !subtitle.is_empty() {
            s.set_subtitle(subtitle);
        }
        self.add(&s);
        s
    }

    pub fn spin(&self, title: &str, min: f64, max: f64, value: f64) -> adw::SpinRow {
        let s = adw::SpinRow::with_range(min, max, 1.0);
        s.set_title(title);
        s.set_value(value);
        self.add(&s);
        s
    }

    pub fn info(&self, title: &str, value: &str) -> adw::ActionRow {
        let r = adw::ActionRow::builder()
            .title(title)
            .subtitle(value)
            .subtitle_selectable(true)
            .build();
        r.add_css_class("property");
        self.add(&r);
        r
    }

    /// Multi-line text area in its own framed box.
    pub fn text(&self, title: &str, initial: &str, height: i32) -> gtk::TextView {
        let g = self.group(title);
        let tv = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        tv.buffer().set_text(initial);
        let sw = gtk::ScrolledWindow::builder()
            .child(&tv)
            .min_content_height(height)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        sw.add_css_class("card");
        g.add(&sw);
        tv
    }

    /// Adds a condition that must hold for OK to be enabled. Call
    /// [`Form::revalidate`] from change handlers.
    pub fn validate(self: &Rc<Self>, f: impl Fn() -> bool + 'static) {
        self.validators.borrow_mut().push(Rc::new(f));
        self.revalidate();
    }

    pub fn revalidate(&self) {
        let ok = self.validators.borrow().iter().all(|v| v());
        self.ok.set_sensitive(ok);
    }

    /// Revalidate whenever this editable changes.
    pub fn watch(self: &Rc<Self>, e: &impl IsA<gtk::Editable>) {
        let f = Rc::downgrade(self);
        e.connect_changed(move |_| {
            if let Some(f) = f.upgrade() {
                f.revalidate();
            }
        });
    }

    pub fn watch_combo(self: &Rc<Self>, c: &adw::ComboRow) {
        let f = Rc::downgrade(self);
        c.connect_selected_notify(move |_| {
            if let Some(f) = f.upgrade() {
                f.revalidate();
            }
        });
    }

    /// Widget that receives keyboard focus when the dialog opens. Without
    /// one, focus goes to the first focusable row.
    pub fn focus(&self, w: &impl IsA<gtk::Widget>) {
        *self.focus.borrow_mut() = Some(w.clone().upcast());
    }

    /// Focus the OK button, so Enter confirms (option-only forms).
    pub fn focus_ok(&self) {
        self.focus(&self.ok.clone());
    }

    /// Focus Cancel, so Enter doesn't trigger a destructive action.
    pub fn focus_cancel(&self) {
        self.focus(&self.cancel.clone());
    }

    /// Presents the dialog and resolves to true when OK was pressed.
    pub async fn run(&self, parent: &impl IsA<gtk::Widget>) -> bool {
        self.dialog.present(Some(parent));
        if std::env::var_os("GITREE_AUTO_ACCEPT").is_some() {
            // Development aid: accept with default values.
            let ok = self.ok.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                if ok.is_sensitive() {
                    ok.emit_clicked();
                }
            });
        }
        let target = self.focus.borrow().clone();
        let first = self.page.first_child();
        glib::idle_add_local_once(move || {
            if let Some(w) = target {
                w.grab_focus();
            } else if let Some(w) = first {
                w.child_focus(gtk::DirectionType::TabForward);
            }
        });
        self.rx.recv().await.unwrap_or(false)
    }
}

/// Selected string of a combo row built with [`Form::combo`].
pub fn combo_value(c: &adw::ComboRow) -> String {
    c.selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|s| s.string().to_string())
        .unwrap_or_default()
}

pub fn text_of(tv: &gtk::TextView) -> String {
    let b = tv.buffer();
    b.text(&b.start_iter(), &b.end_iter(), false).to_string()
}
