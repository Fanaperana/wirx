//! Arranging groups on sheets as a grid of framed blocks, like a hand-drawn schematic: groups that
//! share nets sit next to each other, every row spans the page width, the blocks of a row share
//! one height, and each block's circuit is centred in its frame.

use super::group::GroupOut;
use super::{Item, Paper, SheetOut};
use crate::diag::Diagnostic;
use crate::geom::{pt, Rect, GRID};
use crate::ir::Design;
use std::collections::{HashMap, HashSet};

const MARGIN: i32 = 500;
/// Height kept free at the bottom for the title block.
const TITLE_BLOCK: i32 = 1300;
/// Space between neighbouring frames.
const GUTTER_X: i32 = 150;
/// Vertical space between rows; leaves room for the block names drawn above the frames.
const GUTTER_Y: i32 = 250;
/// Space between a circuit and its frame.
const PAD: i32 = 200;
/// A row may grow this much (relative) to fill the page height.
const MAX_ROW_GROWTH: f64 = 0.0;

fn snap_down(v: i32) -> i32 {
    v.div_euclid(GRID) * GRID
}

fn snap_up(v: i32) -> i32 {
    -(-v).div_euclid(GRID) * GRID
}

struct Boxed {
    out: GroupOut,
    /// The circuit plus padding: the smallest frame this group fits in.
    content: Rect,
    /// Nets this group carries labels for.
    nets: HashSet<usize>,
}

fn boxed(g: GroupOut) -> Boxed {
    let b = g.bbox;
    let content = Rect::new(snap_down(b.x0 - PAD), snap_down(b.y0 - PAD), snap_up(b.x1 + PAD), snap_up(b.y1 + PAD));
    let nets = g.items.iter().filter_map(|it| if let Item::Label { net, .. } = it { Some(*net) } else { None }).collect();
    Boxed { out: g, content, nets }
}

/// Order groups so that groups sharing nets come one after another: start with the biggest, then
/// take the group most connected to the ones already placed and put it on the side its partner's
/// label points to (a `~RESET` label on the left puts the reset block to the left).
fn connection_order(boxes: &[Boxed]) -> Vec<usize> {
    let n = boxes.len();
    let shared = |a: usize, b: usize| boxes[a].nets.intersection(&boxes[b].nets).count();
    let area = |i: usize| boxes[i].content.w() as i64 * boxes[i].content.h() as i64;
    let label_dir = |g: usize, net: usize| -> Option<u16> {
        boxes[g].out.items.iter().find_map(|it| match it {
            Item::Label { net: n, rot, .. } if *n == net => Some(*rot),
            _ => None,
        })
    };
    let first = (0..n).max_by_key(|&i| (area(i), std::cmp::Reverse(i))).unwrap();
    let mut seq: std::collections::VecDeque<usize> = std::collections::VecDeque::from([first]);
    let mut taken = vec![false; n];
    taken[first] = true;
    while seq.len() < n {
        let next = (0..n)
            .filter(|&i| !taken[i])
            .max_by_key(|&i| (seq.iter().map(|&j| shared(i, j)).sum::<usize>(), std::cmp::Reverse(i)))
            .unwrap();
        taken[next] = true;
        let partner = seq.iter().copied().max_by_key(|&j| shared(next, j)).unwrap();
        let net = boxes[next].nets.intersection(&boxes[partner].nets).min().copied();
        let left = net.and_then(|nt| label_dir(partner, nt)) == Some(180);
        let at_front = seq.front() == Some(&partner);
        let at_back = seq.back() == Some(&partner);
        if left && (at_front || !at_back) {
            seq.push_front(next);
        } else {
            seq.push_back(next);
        }
    }
    seq.into_iter().collect()
}

/// Frame of each box (by index into `boxes`), in a `w` × `h` area, or None if they don't fit.
fn tile(boxes: &[Boxed], order: &[usize], w: i32, h: i32) -> Option<HashMap<usize, Rect>> {
    // Rows: fill left to right.
    let mut rows: Vec<Vec<usize>> = vec![];
    let mut width = 0;
    for &i in order {
        let bw = boxes[i].content.w();
        if bw > w {
            return None;
        }
        match rows.last_mut() {
            Some(row) if width + GUTTER_X + bw <= w => {
                row.push(i);
                width += GUTTER_X + bw;
            }
            _ => {
                rows.push(vec![i]);
                width = bw;
            }
        }
    }
    let heights: Vec<i32> = rows.iter().map(|r| r.iter().map(|&i| boxes[i].content.h()).max().unwrap()).collect();
    let used: i32 = heights.iter().sum::<i32>() + GUTTER_Y * rows.len() as i32;
    if used > h {
        return None;
    }
    // Every row gets the width of the widest one (the page width when that is close), so the
    // frames form one aligned grid without blowing a sparse sheet up to the whole page.
    let widest = rows.iter().map(|r| r.iter().map(|&i| boxes[i].content.w()).sum::<i32>() + GUTTER_X * (r.len() as i32 - 1)).max().unwrap_or(0);
    let w = if (w - widest) as f64 <= w as f64 * 0.3 { w } else { widest };
    // Spread the spare height over the rows, within limits.
    let spare = h - used;
    let total: i32 = heights.iter().sum();
    let mut frames = HashMap::new();
    let mut y = GUTTER_Y;
    for (row, &rh) in rows.iter().zip(&heights) {
        let grow = ((spare as f64 * rh as f64 / total as f64).min(rh as f64 * MAX_ROW_GROWTH)) as i32;
        let rh = snap_down(rh + grow);
        let natural: i32 = row.iter().map(|&i| boxes[i].content.w()).sum::<i32>() + GUTTER_X * (row.len() as i32 - 1);
        let extra = (w - natural) / row.len() as i32;
        let mut x = 0;
        for (k, &i) in row.iter().enumerate() {
            let fw = if k + 1 == row.len() { w - x } else { snap_down(boxes[i].content.w() + extra) };
            frames.insert(i, Rect::new(x, y, x + fw, y + rh));
            x += fw + GUTTER_X;
        }
        y += rh + GUTTER_Y;
    }
    Some(frames)
}

fn usable(p: Paper) -> (i32, i32) {
    let (w, h) = p.size();
    (snap_down(w - 2 * MARGIN), snap_down(h - 2 * MARGIN - TITLE_BLOCK))
}

/// Move a group into its frame (circuit centred) and add the frame.
fn emit(b: Boxed, frame: Rect, items: &mut Vec<Item>) {
    let c = b.content;
    let target = pt(frame.x0 + (frame.w() - c.w()) / 2, frame.y0 + (frame.h() - c.h()) / 2);
    let delta = pt(snap_down(target.x - c.x0), snap_down(target.y - c.y0));
    for mut it in b.out.items {
        it.translate(delta);
        items.push(it);
    }
    items.push(Item::Block { name: b.out.name, rect: frame });
}

pub fn pack(d: &Design, groups: Vec<GroupOut>, diags: &mut Vec<Diagnostic>) -> Vec<SheetOut> {
    let mut by_sheet: Vec<Vec<Boxed>> = (0..d.sheets.len()).map(|_| Vec::new()).collect();
    for g in groups {
        let s = d.groups[g.group].sheet;
        by_sheet[s].push(boxed(g));
    }
    let mut sheets: Vec<SheetOut> = Vec::new();
    for (si, boxes) in by_sheet.into_iter().enumerate() {
        if boxes.is_empty() {
            continue;
        }
        let name = d.sheets[si].name.clone();
        let order = connection_order(&boxes);
        let origin = pt(MARGIN, MARGIN);
        let fit = [Paper::A4, Paper::A3, Paper::A2].into_iter().find_map(|p| {
            let (w, h) = usable(p);
            tile(&boxes, &order, w, h).map(|f| (p, f))
        });
        let mut pages: Vec<(Paper, Vec<(Boxed, Rect)>)> = Vec::new();
        match fit {
            Some((paper, mut frames)) => {
                // Centre the grid across the page; keep it at the top.
                let (w, _) = usable(paper);
                let all = frames.values().copied().reduce(Rect::union).unwrap();
                let shift = pt(snap_down((w - all.w()) / 2 - all.x0), 0);
                for f in frames.values_mut() {
                    *f = f.translate(shift);
                }
                let mut list: Vec<(Boxed, Rect)> = boxes.into_iter().enumerate().map(|(i, b)| (b, frames[&i])).collect();
                list.sort_by_key(|(_, f)| (f.y0, f.x0));
                pages.push((paper, list));
            }
            None => {
                // Too much for one A2 page: continue on more A3 pages, in connection order.
                let (w, h) = usable(Paper::A3);
                let mut slots: Vec<Option<Boxed>> = boxes.into_iter().map(Some).collect();
                let mut current: Vec<usize> = Vec::new();
                let flush = |current: &mut Vec<usize>, slots: &mut Vec<Option<Boxed>>, pages: &mut Vec<(Paper, Vec<(Boxed, Rect)>)>, diags: &mut Vec<Diagnostic>| {
                    if current.is_empty() {
                        return;
                    }
                    let sub: Vec<Boxed> = current.iter().map(|&i| slots[i].take().unwrap()).collect();
                    let sub_order: Vec<usize> = (0..sub.len()).collect();
                    let (paper, frames) = match tile(&sub, &sub_order, w, h) {
                        Some(f) => (Paper::A3, f),
                        None => {
                            diags.push(Diagnostic::warning("sheet_overflow", "a group is larger than an A3 page; it was put on an A2 page"));
                            let (w2, h2) = usable(Paper::A2);
                            let f = tile(&sub, &sub_order, w2, h2.max(sub.iter().map(|b| b.content.h()).sum::<i32>() + 2000)).unwrap_or_default();
                            (Paper::A2, f)
                        }
                    };
                    pages.push((paper, sub.into_iter().enumerate().map(|(i, b)| (b, frames.get(&i).copied().unwrap_or(Rect::new(0, 0, 100, 100)))).collect()));
                    current.clear();
                };
                for &i in &order {
                    current.push(i);
                    let fits = {
                        let sizes: Vec<&Boxed> = current.iter().map(|&j| slots[j].as_ref().unwrap()).collect();
                        let tmp: Vec<Boxed> = sizes
                            .iter()
                            .map(|b| Boxed { out: GroupOut { group: b.out.group, name: String::new(), items: vec![], bbox: b.out.bbox }, content: b.content, nets: HashSet::new() })
                            .collect();
                        let o: Vec<usize> = (0..tmp.len()).collect();
                        tile(&tmp, &o, w, h).is_some()
                    };
                    if !fits && current.len() > 1 {
                        current.pop();
                        flush(&mut current, &mut slots, &mut pages, diags);
                        current.push(i);
                    }
                }
                flush(&mut current, &mut slots, &mut pages, diags);
            }
        }
        let count = pages.len();
        for (pi, (paper, list)) in pages.into_iter().enumerate() {
            let mut items = Vec::new();
            for (b, frame) in list {
                emit(b, frame.translate(origin), &mut items);
            }
            let sheet_name = if count > 1 { format!("{name} ({})", pi + 1) } else { name.clone() };
            sheets.push(SheetOut { name: sheet_name, paper, items });
        }
    }

    // Labels whose net appears on more than one sheet become global labels.
    let mut sheets_of: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (si, s) in sheets.iter().enumerate() {
        for it in &s.items {
            if let Item::Label { net, .. } = it {
                sheets_of.entry(*net).or_default().insert(si);
            }
        }
    }
    for s in &mut sheets {
        for it in &mut s.items {
            if let Item::Label { net, global, .. } = it {
                *global = sheets_of.get(net).is_some_and(|set| set.len() > 1);
            }
        }
    }
    sheets
}
