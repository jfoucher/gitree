//! Commit graph lane assignment.
//!
//! Commits are fed in log order (children before parents). Each row records
//! the column of its node and the line segments to draw in that row:
//! - `Full`: a lane passing straight through (top to bottom),
//! - `Top`: a lane from the top of the row converging into the node,
//! - `Bottom`: from the node down to a lane at the bottom of the row.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    Full,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub color: u8,
    pub half: Half,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub col: u16,
    pub color: u8,
    pub edges: Vec<Edge>,
    /// Number of lane columns used by this row.
    pub width: u16,
    pub is_merge: bool,
}

pub const PALETTE_SIZE: u8 = 8;

#[derive(Default)]
pub struct GraphBuilder {
    lanes: Vec<Option<(String, u8)>>,
    next_color: u8,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn new_color(&mut self) -> u8 {
        let c = self.next_color;
        self.next_color = (self.next_color + 1) % PALETTE_SIZE;
        c
    }

    fn free_slot(&mut self) -> usize {
        match self.lanes.iter().position(|l| l.is_none()) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    pub fn push(&mut self, oid: &str, parents: &[String]) -> GraphRow {
        let matches: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|(o, _)| o == oid))
            .map(|(i, _)| i)
            .collect();

        let (col, color) = match matches.first() {
            Some(&i) => (i, self.lanes[i].as_ref().unwrap().1),
            None => {
                let i = self.free_slot();
                let c = self.new_color();
                (i, c)
            }
        };

        let mut edges = Vec::new();
        for (i, lane) in self.lanes.iter().enumerate() {
            if let Some((o, c)) = lane {
                if o == oid {
                    edges.push(Edge {
                        from: i as u16,
                        to: col as u16,
                        color: *c,
                        half: Half::Top,
                    });
                } else {
                    edges.push(Edge {
                        from: i as u16,
                        to: i as u16,
                        color: *c,
                        half: Half::Full,
                    });
                }
            }
        }
        let width_before = self.lanes.len();

        for &m in &matches {
            self.lanes[m] = None;
        }
        if col >= self.lanes.len() {
            self.lanes.resize(col + 1, None);
        }

        if let Some(p0) = parents.first() {
            self.lanes[col] = Some((p0.clone(), color));
            edges.push(Edge {
                from: col as u16,
                to: col as u16,
                color,
                half: Half::Bottom,
            });
        }
        for p in parents.iter().skip(1) {
            let existing = self
                .lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(o, _)| o == p));
            let (k, c) = match existing {
                Some(k) => (k, self.lanes[k].as_ref().unwrap().1),
                None => {
                    let k = self.free_slot();
                    let c = self.new_color();
                    self.lanes[k] = Some((p.clone(), c));
                    (k, c)
                }
            };
            edges.push(Edge {
                from: col as u16,
                to: k as u16,
                color: c,
                half: Half::Bottom,
            });
        }

        while self.lanes.last().is_some_and(|l| l.is_none()) {
            self.lanes.pop();
        }
        let width = width_before.max(self.lanes.len()).max(col + 1) as u16;
        GraphRow {
            col: col as u16,
            color,
            edges,
            width,
            is_merge: parents.len() > 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn linear_history_stays_in_column_zero() {
        let mut g = GraphBuilder::new();
        let r1 = g.push("c", &p(&["b"]));
        let r2 = g.push("b", &p(&["a"]));
        let r3 = g.push("a", &[]);
        for r in [&r1, &r2, &r3] {
            assert_eq!(r.col, 0);
        }
        assert_eq!(r3.edges.len(), 1); // only the top edge into a
        assert_eq!(r3.edges[0].half, Half::Top);
    }

    #[test]
    fn merge_and_branch() {
        // m merges x into b; both descend from a.
        let mut g = GraphBuilder::new();
        let m = g.push("m", &p(&["b", "x"]));
        assert!(m.is_merge);
        assert_eq!(m.col, 0);
        assert!(m.edges.iter().any(|e| e.half == Half::Bottom && e.to == 1));
        let x = g.push("x", &p(&["a"]));
        assert_eq!(x.col, 1);
        let b = g.push("b", &p(&["a"]));
        assert_eq!(b.col, 0);
        // Lane 1 (x -> a) passes through b's row.
        assert!(b.edges.iter().any(|e| e.half == Half::Full && e.from == 1));
        let a = g.push("a", &[]);
        assert_eq!(a.col, 0);
        // Two lanes converge into a.
        assert_eq!(a.edges.iter().filter(|e| e.half == Half::Top).count(), 2);
    }

    #[test]
    fn independent_tips_get_new_lanes() {
        let mut g = GraphBuilder::new();
        let t1 = g.push("t1", &p(&["a"]));
        let t2 = g.push("t2", &p(&["a"]));
        assert_eq!(t1.col, 0);
        assert_eq!(t2.col, 1);
        assert_ne!(t1.color, t2.color);
        let a = g.push("a", &[]);
        assert_eq!(a.col, 0);
        assert_eq!(a.width, 2);
    }

    #[test]
    fn octopus_merge() {
        let mut g = GraphBuilder::new();
        let m = g.push("m", &p(&["a", "b", "c"]));
        assert_eq!(m.edges.iter().filter(|e| e.half == Half::Bottom).count(), 3);
        assert_eq!(m.width, 3);
    }
}
