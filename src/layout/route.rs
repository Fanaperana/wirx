//! Orthogonal wire router on the 50 mil grid (Dijkstra over cell × direction).
//!
//! eleceda connects anything whose point lies on a wire, so the router keeps nets apart strictly:
//! a wire may cross another net's wire only by passing straight through, never bends or ends on
//! it, and never runs along it. Pin tips are entered only from outside the part.

use crate::geom::{dir_vec, opposite, pt, Pt, Rect, Rot, GRID};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

pub const FREE: i32 = -1;
/// A point used by something that must not join any net (unconnected pin, no-connect flag).
pub const FOREIGN: i32 = -2;

const STEP: u32 = 10;
const BEND: u32 = 80;
const CROSS: u32 = 60;
/// Running right next to another net's parallel wire, when `hug` is on.
const HUG: u32 = 25;

#[derive(Clone)]
pub struct Grid {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    blocked: Vec<bool>,
    soft: Vec<u16>,
    owner: Vec<i32>,
    hnet: Vec<i32>,
    vnet: Vec<i32>,
    /// Pin tips: a wire must arrive moving in this direction.
    access: HashMap<usize, Rot>,
    /// Extra cost per bend (cleanup uses it to straighten jogs).
    pub extra_bend: u32,
    /// Charge for running alongside another net's parallel wire (cleanup: track separation).
    pub hug: bool,
}

const DIRS: [Rot; 4] = [0, 90, 180, 270];
fn dir_index(d: Rot) -> usize {
    (d / 90) as usize
}

impl Grid {
    pub fn new(bbox: Rect) -> Grid {
        let x0 = bbox.x0.div_euclid(GRID) * GRID;
        let y0 = bbox.y0.div_euclid(GRID) * GRID;
        let w = ((bbox.x1 - x0) / GRID + 1) as usize;
        let h = ((bbox.y1 - y0) / GRID + 1) as usize;
        let n = w * h;
        Grid {
            x0,
            y0,
            w,
            h,
            blocked: vec![false; n],
            soft: vec![0; n],
            owner: vec![FREE; n],
            hnet: vec![FREE; n],
            vnet: vec![FREE; n],
            access: HashMap::new(),
            extra_bend: 0,
            hug: false,
        }
    }

    pub fn idx(&self, p: Pt) -> Option<usize> {
        if (p.x - self.x0) % GRID != 0 || (p.y - self.y0) % GRID != 0 {
            return None;
        }
        let cx = (p.x - self.x0) / GRID;
        let cy = (p.y - self.y0) / GRID;
        if cx < 0 || cy < 0 || cx as usize >= self.w || cy as usize >= self.h {
            return None;
        }
        Some(cy as usize * self.w + cx as usize)
    }

    pub fn pt(&self, i: usize) -> Pt {
        pt(self.x0 + (i % self.w) as i32 * GRID, self.y0 + (i / self.w) as i32 * GRID)
    }

    fn cells_in(&self, r: Rect) -> Vec<usize> {
        let mut out = Vec::new();
        let gx0 = (r.x0 - self.x0).div_euclid(GRID).max(0);
        let gy0 = (r.y0 - self.y0).div_euclid(GRID).max(0);
        let gx1 = ((r.x1 - self.x0).div_euclid(GRID)).min(self.w as i32 - 1);
        let gy1 = ((r.y1 - self.y0).div_euclid(GRID)).min(self.h as i32 - 1);
        for cy in gy0..=gy1 {
            for cx in gx0..=gx1 {
                let p = pt(self.x0 + cx * GRID, self.y0 + cy * GRID);
                if r.contains(p) {
                    out.push(cy as usize * self.w + cx as usize);
                }
            }
        }
        out
    }

    pub fn block_rect(&mut self, r: Rect) {
        for i in self.cells_in(r) {
            self.blocked[i] = true;
        }
    }

    /// Extra cost for one cell (congestion history).
    pub fn add_soft(&mut self, cell: usize, cost: u16) {
        self.soft[cell] = self.soft[cell].saturating_add(cost);
    }

    pub fn soft_rect(&mut self, r: Rect, cost: u16) {
        for i in self.cells_in(r) {
            self.soft[i] = self.soft[i].max(cost);
        }
    }

    /// A connection point of `net` (or FOREIGN). Overrides blocking, since such points sit on edges.
    pub fn own(&mut self, p: Pt, net: i32) {
        if let Some(i) = self.idx(p) {
            self.owner[i] = net;
            self.blocked[i] = false;
        }
    }

    pub fn set_access(&mut self, p: Pt, arrive: Rot) {
        if let Some(i) = self.idx(p) {
            self.access.insert(i, arrive);
        }
    }

    /// Cells of the axis-aligned segment ab.
    pub fn segment_cells(&self, a: Pt, b: Pt) -> Vec<usize> {
        let n = (a.manhattan(b) / GRID).max(0);
        let step = pt((b.x - a.x).signum() * GRID, (b.y - a.y).signum() * GRID);
        (0..=n).filter_map(|k| self.idx(a + step * k)).collect()
    }

    pub fn mark_wire(&mut self, a: Pt, b: Pt, net: i32) {
        let horizontal = a.y == b.y;
        for i in self.segment_cells(a, b) {
            if horizontal { self.hnet[i] = net } else { self.vnet[i] = net }
            self.blocked[i] = false;
        }
        self.own(a, net);
        self.own(b, net);
    }

    /// Cheapest path for `net` from a pin tip leaving in direction `out` to any target cell.
    /// Returns corner points from start to end.
    pub fn route(&self, net: i32, start: Pt, out: Rot, targets: &HashSet<usize>) -> Option<Vec<Pt>> {
        let s = self.idx(start)?;
        let n = self.w * self.h;
        let mut dist = vec![u32::MAX; n * 4];
        let mut prev = vec![usize::MAX; n * 4];
        let mut heap = BinaryHeap::new();

        let crossing = |c: usize, d: Rot| -> bool {
            let other = |v: i32| v != FREE && v != net;
            if d == 0 || d == 180 { other(self.vnet[c]) } else { other(self.hnet[c]) }
        };
        let enter = |c: usize, d: Rot| -> Option<u32> {
            let target = targets.contains(&c);
            if target {
                if let Some(&a) = self.access.get(&c)
                    && a != d
                {
                    return None;
                }
                return Some(0);
            }
            if self.blocked[c] || self.owner[c] != FREE {
                return None;
            }
            let along = if d == 0 || d == 180 { self.hnet[c] } else { self.vnet[c] };
            if along != FREE {
                return None;
            }
            let hug = self.hug && {
                let horizontal = d == 0 || d == 180;
                let (x, y) = (c % self.w, c / self.w);
                let foreign = |dx: i64, dy: i64| -> bool {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= self.w as i64 || ny >= self.h as i64 {
                        return false;
                    }
                    let i = ny as usize * self.w + nx as usize;
                    let v = if horizontal { self.hnet[i] } else { self.vnet[i] };
                    v != FREE && v != net
                };
                if horizontal { foreign(0, -1) || foreign(0, 1) } else { foreign(-1, 0) || foreign(1, 0) }
            };
            Some(self.soft[c] as u32 + if crossing(c, d) { CROSS } else { 0 } + if hug { HUG } else { 0 })
        };
        let step = |c: usize, d: Rot| -> Option<usize> { self.idx(self.pt(c) + dir_vec(d) * GRID) };

        let first = step(s, out)?;
        let c0 = enter(first, out)?;
        let st = first * 4 + dir_index(out);
        dist[st] = STEP + c0;
        prev[st] = usize::MAX - 1; // marks "came from start"
        heap.push(Reverse((STEP + c0, st)));

        let mut goal = None;
        while let Some(Reverse((cost, state))) = heap.pop() {
            if cost > dist[state] {
                continue;
            }
            let (c, d) = (state / 4, DIRS[state % 4]);
            if targets.contains(&c) {
                goal = Some(state);
                break;
            }
            let straight_only = crossing(c, d);
            for nd in DIRS {
                if nd == opposite(d) || (straight_only && nd != d) {
                    continue;
                }
                let Some(nc) = step(c, nd) else { continue };
                let Some(extra) = enter(nc, nd) else { continue };
                let nc_cost = cost + STEP + extra + if nd != d { BEND + self.extra_bend } else { 0 };
                let ns = nc * 4 + dir_index(nd);
                if nc_cost < dist[ns] {
                    dist[ns] = nc_cost;
                    prev[ns] = state;
                    heap.push(Reverse((nc_cost, ns)));
                }
            }
        }
        let goal = goal?;
        let mut cells = vec![goal / 4];
        let mut cur = goal;
        while prev[cur] != usize::MAX - 1 {
            cur = prev[cur];
            cells.push(cur / 4);
        }
        cells.push(s);
        cells.reverse();
        // Keep corners only.
        let pts: Vec<Pt> = cells.iter().map(|&c| self.pt(c)).collect();
        let mut out_pts = vec![pts[0]];
        for k in 1..pts.len() - 1 {
            let (a, b, c) = (pts[k - 1], pts[k], pts[k + 1]);
            let collinear = (a.x == b.x && b.x == c.x) || (a.y == b.y && b.y == c.y);
            if !collinear {
                out_pts.push(b);
            }
        }
        out_pts.push(*pts.last().unwrap());
        Some(out_pts)
    }
}
