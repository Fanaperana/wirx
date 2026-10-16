//! Arranging groups on sheets as framed blocks, like a hand-drawn schematic. A sheet with a big
//! group and partners it shares labels with is drawn around that group, the partners beside the
//! pins they connect to (see [`hub`]); otherwise the blocks form a grid where groups that share
//! nets sit next to each other, every row spans the page width, the blocks of a row share one
//! height, and each block's circuit is centred in its frame.

use super::group::GroupOut;
use super::{Item, Paper, SheetOut};
use crate::diag::Diagnostic;
use crate::geom::{pt, Pt, Rect, GRID};
use crate::ir::Design;
use crate::symbol::SymbolDef;
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
    /// Those of them whose label sits on a small part's pin.
    small: HashSet<usize>,
}

fn boxed(d: &Design, g: GroupOut) -> Boxed {
    let b = g.bbox;
    let content = Rect::new(snap_down(b.x0 - PAD), snap_down(b.y0 - PAD), snap_up(b.x1 + PAD), snap_up(b.y1 + PAD));
    let nets = g.items.iter().filter_map(|it| if let Item::Label { net, .. } = it { Some(*net) } else { None }).collect();
    let small = super::join::small_label_nets(d, &g.items);
    Boxed { out: g, content, nets, small }
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

/// Where a group's labels for `nets` sit, in its own coordinates: mean point and the direction most
/// of them leave in.
fn labels_of(b: &Boxed, nets: &HashSet<usize>) -> Option<(Pt, u16)> {
    let found: Vec<(Pt, u16)> = b.out.items.iter().filter_map(|it| match it {
        Item::Label { net, at, rot, .. } if nets.contains(net) => Some((*at, *rot)),
        _ => None,
    }).collect();
    if found.is_empty() {
        return None;
    }
    let n = found.len() as i32;
    let mean = pt(found.iter().map(|f| f.0.x).sum::<i32>() / n, found.iter().map(|f| f.0.y).sum::<i32>() / n);
    let mut votes: HashMap<u16, usize> = HashMap::new();
    for f in &found {
        *votes.entry(f.1).or_default() += 1;
    }
    let rot = votes.into_iter().max_by_key(|&(r, c)| (c, std::cmp::Reverse(r))).unwrap().0;
    Some((mean, rot))
}

/// A sheet drawn around its biggest group, the way a chip's page is drawn by hand: the groups it
/// shares labels with stand in a column beside the side those labels leave from (the crystal
/// left of the chip when XIN is on its left), each level with its labels, and frames fit their
/// circuits. Groups wired only to a column member join that column next to it; the rest go in a
/// row underneath. Frames are relative to the top-left of the arrangement; None if there is no
/// side partner (the plain grid is then the better drawing) or it is wider than `w` or taller
/// than `h`.
fn hub(boxes: &[Boxed], w: i32, h: i32) -> Option<HashMap<usize, Rect>> {
    let n = boxes.len();
    if n < 2 {
        return None;
    }
    let area = |i: usize| boxes[i].content.w() as i64 * boxes[i].content.h() as i64;
    let centre = (0..n).max_by_key(|&i| (area(i), std::cmp::Reverse(i))).unwrap();
    let shared = |a: usize, b: usize| -> HashSet<usize> { boxes[a].nets.intersection(&boxes[b].nets).copied().collect() };
    let hc = boxes[centre].content;
    let hub_frame = Rect::new(0, 0, hc.w(), hc.h());
    // Group-local point → arrangement coordinates for a group whose frame top-left is `at`.
    let local = |i: usize, at: Pt, p: Pt| pt(at.x + p.x - boxes[i].content.x0, at.y + p.y - boxes[i].content.y0);

    // Attach groups in order of how strongly they connect to what is already attached.
    // side: (180 left or 0 right, wanted frame top, partner).
    let mut side: HashMap<usize, (u16, i32, usize)> = HashMap::new();
    let mut loose: Vec<usize> = Vec::new();
    let mut done = vec![false; n];
    done[centre] = true;
    for _ in 1..n {
        let next = (0..n)
            .filter(|&i| !done[i])
            .max_by_key(|&i| ((0..n).filter(|&j| j == centre || side.contains_key(&j)).map(|j| shared(i, j).len()).sum::<usize>(), std::cmp::Reverse(i)))
            .unwrap();
        done[next] = true;
        let partner = (0..n)
            .filter(|&j| j == centre || side.contains_key(&j))
            .max_by_key(|&j| (shared(next, j).len(), j == centre, std::cmp::Reverse(j)))
            .filter(|&j| !shared(next, j).is_empty());
        let Some(partner) = partner else {
            loose.push(next);
            continue;
        };
        let nets = shared(next, partner);
        let (Some((theirs, rot)), Some((mine, _))) = (labels_of(&boxes[partner], &nets), labels_of(&boxes[next], &nets)) else {
            loose.push(next);
            continue;
        };
        let (s, partner_top) = if partner == centre {
            let s = match rot {
                180 => 180,
                0 => 0,
                _ if theirs.x < hc.center().x => 180,
                _ => 0,
            };
            (s, hub_frame.y0)
        } else {
            (side[&partner].0, side[&partner].1)
        };
        let theirs = local(partner, pt(0, partner_top), theirs).y;
        let mine = mine.y - boxes[next].content.y0;
        side.insert(next, (s, snap_down(theirs - mine), partner));
    }
    if side.is_empty() {
        return None;
    }

    let mut frames: HashMap<usize, Rect> = HashMap::new();
    frames.insert(centre, hub_frame);
    for s in [180u16, 0] {
        let mut col: Vec<usize> = side.iter().filter(|(_, v)| v.0 == s).map(|(&i, _)| i).collect();
        col.sort_by_key(|&i| (side[&i].1, i));
        if col.is_empty() {
            continue;
        }
        let frame_at = |i: usize, top: i32| {
            let c = boxes[i].content;
            let x = if s == 180 { hub_frame.x0 - GUTTER_X - c.w() } else { hub_frame.x1 + GUTTER_X };
            Rect::new(x, top, x + c.w(), top + c.h())
        };
        let small = |i: usize, p: usize, nt: usize| boxes[i].small.contains(&nt) || boxes[p].small.contains(&nt);
        // A block whose labels sit on small parts (a crystal's resistor, a button) is read through
        // them: it weighs more when the column settles.
        let weight = |i: usize| {
            let p = side[&i].2;
            if shared(i, p).iter().any(|&nt| small(i, p, nt)) { 10i64 } else { 1 }
        };
        // Tops for one order: each run of touching blocks settles at the weighted mean of what its
        // members want (pool adjacent violators).
        let settle = |order: &[usize]| -> Vec<i32> {
            let mut clusters: Vec<(Vec<usize>, i32)> = Vec::new();
            let height = |m: &[usize]| m.iter().map(|&i| boxes[i].content.h()).sum::<i32>() + GUTTER_Y * (m.len() as i32 - 1);
            let ideal = |m: &[usize]| {
                let (mut off, mut sum, mut wsum) = (0, 0i64, 0i64);
                for &i in m {
                    sum += weight(i) * (side[&i].1 - off) as i64;
                    wsum += weight(i);
                    off += boxes[i].content.h() + GUTTER_Y;
                }
                snap_down((sum / wsum) as i32)
            };
            for &i in order {
                clusters.push((vec![i], side[&i].1));
                while clusters.len() > 1 {
                    let k = clusters.len();
                    if clusters[k - 1].1 >= clusters[k - 2].1 + height(&clusters[k - 2].0) + GUTTER_Y {
                        break;
                    }
                    let mut m = clusters[k - 2].0.clone();
                    m.extend(clusters[k - 1].0.iter().copied());
                    let top = ideal(&m);
                    clusters.truncate(k - 2);
                    clusters.push((m, top));
                }
            }
            let mut tops = Vec::new();
            for (m, top) in clusters {
                let mut y = top;
                for i in m {
                    tops.push(y);
                    y += boxes[i].content.h() + GUTTER_Y;
                }
            }
            tops
        };
        // Cost of an order: how far each label is from its partner's (far labels on small parts
        // cost most), plus anything that would not fit the page height.
        let cost = |order: &[usize], tops: &[i32]| -> i64 {
            let at: HashMap<usize, Rect> = order.iter().zip(tops).map(|(&i, &t)| (i, frame_at(i, t))).collect();
            let mut c = 0i64;
            for &i in order {
                let p = side[&i].2;
                let pf = if p == centre { hub_frame } else { at[&p] };
                for nt in shared(i, p) {
                    let one = HashSet::from([nt]);
                    let (Some((mine, _)), Some((theirs, _))) = (labels_of(&boxes[i], &one), labels_of(&boxes[p], &one)) else { continue };
                    let d = local(i, pt(at[&i].x0, at[&i].y0), mine).manhattan(local(p, pt(pf.x0, pf.y0), theirs)) as i64;
                    c += d;
                    if small(i, p, nt) {
                        c += 20 * (d - 1500).max(0);
                    }
                }
            }
            let top = at.values().map(|r| r.y0).min().unwrap().min(hub_frame.y0);
            let bottom = at.values().map(|r| r.y1).max().unwrap().max(hub_frame.y1);
            c + 100 * (bottom - top + GUTTER_Y - h).max(0) as i64
        };
        let mut best: Option<(i64, Vec<usize>, Vec<i32>)> = None;
        let mut consider = |order: &[usize]| {
            let tops = settle(order);
            let c = cost(order, &tops);
            if best.as_ref().is_none_or(|b| c < b.0) {
                best = Some((c, order.to_vec(), tops));
            }
        };
        if col.len() <= 6 {
            permutations(&col, &mut consider);
        } else {
            consider(&col);
        }
        let (_, order, tops) = best.unwrap();
        for (&i, &t) in order.iter().zip(&tops) {
            frames.insert(i, frame_at(i, t));
        }
    }
    let all = frames.values().copied().reduce(Rect::union).unwrap();
    if all.w() > w {
        return None;
    }
    // The rest: one row underneath, left to right, wrapping at the page width.
    let (mut x, mut y, mut row_h) = (all.x0, all.y1 + GUTTER_Y, 0);
    for i in loose {
        let c = boxes[i].content;
        if c.w() > w {
            return None;
        }
        if x > all.x0 && x + c.w() > all.x0 + w {
            x = all.x0;
            y += row_h + GUTTER_Y;
            row_h = 0;
        }
        frames.insert(i, Rect::new(x, y, x + c.w(), y + c.h()));
        x += c.w() + GUTTER_X;
        row_h = row_h.max(c.h());
    }
    let all = frames.values().copied().reduce(Rect::union).unwrap();
    if all.h() + GUTTER_Y > h || all.w() > w {
        return None;
    }
    let shift = pt(-all.x0, GUTTER_Y - all.y0);
    Some(frames.into_iter().map(|(i, f)| (i, f.translate(shift))).collect())
}

/// Every order of `items` (in a fixed, deterministic sequence), the given order first.
fn permutations(items: &[usize], f: &mut impl FnMut(&[usize])) {
    fn go(prefix: &mut Vec<usize>, rest: &mut Vec<usize>, f: &mut impl FnMut(&[usize])) {
        if rest.is_empty() {
            f(prefix);
            return;
        }
        for k in 0..rest.len() {
            let x = rest.remove(k);
            prefix.push(x);
            go(prefix, rest, f);
            prefix.pop();
            rest.insert(k, x);
        }
    }
    go(&mut Vec::new(), &mut items.to_vec(), f);
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

pub fn pack(d: &Design, groups: Vec<GroupOut>, extra: &[SymbolDef], diags: &mut Vec<Diagnostic>) -> Vec<SheetOut> {
    // Labels per net in the whole design, for wiring blocks together.
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for g in &groups {
        for it in &g.items {
            if let Item::Label { net, .. } = it {
                *counts.entry(*net).or_default() += 1;
            }
        }
    }
    let mut by_sheet: Vec<Vec<Boxed>> = (0..d.sheets.len()).map(|_| Vec::new()).collect();
    for g in groups {
        let s = d.groups[g.group].sheet;
        by_sheet[s].push(boxed(d, g));
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
            hub(&boxes, w, h).or_else(|| tile(&boxes, &order, w, h)).map(|f| (p, f))
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
                            .map(|b| Boxed { out: GroupOut { group: b.out.group, name: String::new(), items: vec![], bbox: b.out.bbox, stats: Default::default(), positions: Default::default() }, content: b.content, nets: HashSet::new(), small: HashSet::new() })
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
            super::join::join_labels(d, &mut items, extra, &mut counts);
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
