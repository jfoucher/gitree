//! Drawing of one row of the commit graph.

use crate::git::graph::{GraphRow, Half};
use gtk::cairo;

pub const LANE_W: f64 = 14.0;
pub const ROW_H: i32 = 24;
const PAD: f64 = 8.0;

const PALETTE: [(f64, f64, f64); 8] = [
    (0.21, 0.52, 0.89), // blue
    (0.18, 0.76, 0.49), // green
    (1.00, 0.47, 0.00), // orange
    (0.57, 0.25, 0.67), // purple
    (0.88, 0.11, 0.14), // red
    (0.11, 0.63, 0.66), // teal
    (0.90, 0.65, 0.04), // yellow
    (0.75, 0.38, 0.80), // pink
];

pub fn color(i: u8) -> (f64, f64, f64) {
    PALETTE[i as usize % PALETTE.len()]
}

fn x(col: u16) -> f64 {
    PAD + col as f64 * LANE_W
}

pub fn width_for(lanes: u16) -> i32 {
    (PAD * 2.0 + lanes.max(1) as f64 * LANE_W) as i32
}

pub struct NodeStyle {
    pub is_head: bool,
    pub uncommitted: bool,
}

pub fn draw(cr: &cairo::Context, h: f64, row: &GraphRow, style: &NodeStyle) {
    cr.set_line_width(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    let mid = h / 2.0;
    for e in &row.edges {
        let (r, g, b) = color(e.color);
        cr.set_source_rgb(r, g, b);
        let (xf, xt) = (x(e.from), x(e.to));
        match e.half {
            Half::Full => {
                cr.move_to(xf, 0.0);
                cr.line_to(xt, h);
            }
            Half::Top => {
                cr.move_to(xf, -0.5);
                if e.from == e.to {
                    cr.line_to(xt, mid);
                } else {
                    cr.curve_to(xf, mid * 0.6, xt, mid * 0.4, xt, mid);
                }
            }
            Half::Bottom => {
                cr.move_to(xf, mid);
                if e.from == e.to {
                    cr.line_to(xt, h + 0.5);
                } else {
                    cr.curve_to(xf, mid + mid * 0.6, xt, mid + mid * 0.4, xt, h + 0.5);
                }
            }
        }
        if style.uncommitted && e.half == Half::Bottom {
            cr.set_dash(&[3.0, 3.0], 0.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        } else {
            let _ = cr.stroke();
        }
    }

    let (r, g, b) = color(row.color);
    let cx = x(row.col);
    if style.uncommitted {
        cr.set_source_rgb(r, g, b);
        cr.set_dash(&[2.0, 2.0], 0.0);
        cr.arc(cx, mid, 4.5, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        return;
    }
    let radius = if style.is_head { 5.5 } else { 4.0 };
    cr.arc(cx, mid, radius, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(r, g, b);
    if style.is_head {
        let _ = cr.fill_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        cr.arc(cx, mid, 2.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill();
    } else if row.is_merge {
        let _ = cr.fill();
        cr.arc(cx, mid, 1.6, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill();
    } else {
        let _ = cr.fill();
    }
}
