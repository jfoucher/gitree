//! Split panes whose sizes persist across sessions.

use crate::config;
use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Which child of a paned keeps its size when the paned is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Start,
    End,
}

fn extent(p: &gtk::Paned) -> i32 {
    match p.orientation() {
        gtk::Orientation::Horizontal => p.width(),
        _ => p.height(),
    }
}

/// Sizes the `keep` child of `paned` from the size saved under `key` (or
/// `default`), makes that child keep its size when the paned is resized, and
/// saves the size whenever the handle moves.
pub fn remember(paned: &gtk::Paned, key: &'static str, keep: Keep, default: i32) {
    let size = config::with(|s| s.pane_sizes.get(key).copied()).unwrap_or(default);
    paned.set_resize_start_child(keep == Keep::End);
    paned.set_resize_end_child(keep == Keep::Start);

    // An end size only becomes a position once the paned has been allocated.
    let restored = Rc::new(Cell::new(keep == Keep::Start));
    match keep {
        Keep::Start => paned.set_position(size),
        Keep::End => {
            let r = restored.clone();
            paned.connect_map(move |p| {
                if r.get() {
                    return;
                }
                let r = r.clone();
                p.add_tick_callback(move |p, _| {
                    let e = extent(p);
                    if e <= 0 {
                        return glib::ControlFlow::Continue;
                    }
                    p.set_position(e - size);
                    r.set(true);
                    glib::ControlFlow::Break
                });
            });
        }
    }

    // Save once the handle settles rather than on every drag step. A
    // position at the paned's limit is almost always GTK clamping it because
    // the paned got too small, not a size the user chose, so it isn't saved.
    let generation = Rc::new(Cell::new(0u32));
    paned.connect_position_notify(move |p| {
        let pos = p.position();
        if !restored.get() || pos <= p.min_position() || pos >= p.max_position() {
            return;
        }
        let size = match keep {
            Keep::Start => pos,
            Keep::End => {
                let e = extent(p);
                if !p.is_mapped() || e <= 0 {
                    return;
                }
                e - pos
            }
        };
        let current = generation.get().wrapping_add(1);
        generation.set(current);
        let generation = generation.clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if generation.get() == current && config::with(|s| s.pane_sizes.get(key) != Some(&size)) {
                config::update(|s| {
                    s.pane_sizes.insert(key.to_string(), size);
                });
            }
        });
    });
}
