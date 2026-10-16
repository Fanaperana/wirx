//! Packing groups onto sheets: rows of groups, left to right, on the smallest paper that fits.

use super::group::GroupOut;
use super::{Item, Paper, SheetOut};
use crate::diag::Diagnostic;
use crate::geom::{pt, snap, Pt, Rect, GRID};
use crate::ir::Design;
use std::collections::{HashMap, HashSet};

const MARGIN: i32 = 600;
/// Height kept free at the bottom for the title block.
const TITLE_BLOCK: i32 = 1300;
const GAP: i32 = 250;
const FRAME_PAD: i32 = 150;
const FRAME_NAME: i32 = 100;

struct Boxed {
    out: GroupOut,
    /// Everything the group occupies, frame included, before translation.
    total: Rect,
    frame: Option<Rect>,
}

fn boxed(g: GroupOut) -> Boxed {
    let snap_down = |v: i32| v.div_euclid(GRID) * GRID;
    let snap_up = |v: i32| -(-v).div_euclid(GRID) * GRID;
    let b = g.bbox;
    let frame = g.framed.then(|| Rect::new(snap_down(b.x0 - FRAME_PAD), snap_down(b.y0 - FRAME_PAD), snap_up(b.x1 + FRAME_PAD), snap_up(b.y1 + FRAME_PAD)));
    let total = match frame {
        Some(f) => Rect::new(f.x0, f.y0 - FRAME_NAME, f.x1, f.y1),
        None => Rect::new(snap_down(b.x0), snap_down(b.y0), snap_up(b.x1), snap_up(b.y1)),
    };
    Boxed { out: g, total, frame }
}

/// Shelf packing; positions are the top-left corner for each box, or None if it does not fit.
fn shelf(sizes: &[(i32, i32)], w: i32, h: i32) -> Option<Vec<Pt>> {
    let mut out = Vec::new();
    let (mut x, mut y, mut row_h) = (0, 0, 0);
    for &(bw, bh) in sizes {
        if bw > w {
            return None;
        }
        if x > 0 && x + bw > w {
            x = 0;
            y += row_h + GAP;
            row_h = 0;
        }
        if y + bh > h {
            return None;
        }
        out.push(pt(x, y));
        x += bw + GAP;
        row_h = row_h.max(bh);
    }
    Some(out)
}

/// Distance kept between the labels of a net in two neighbouring groups: close enough that the
/// parts read as related, far enough that two labels do not look like a missing wire.
const PARTNER: i32 = 1100;

/// Place groups in connection order: each next group goes as close as possible to the group it
/// shares the most labels with, its label facing the partner's. Positions are top-left corners.
fn connected(boxes: &[Boxed], w: i32, h: i32) -> Option<Vec<Pt>> {
    use crate::geom::dir_vec;
    let n = boxes.len();
    let labels: Vec<Vec<(usize, Pt, u16)>> = boxes
        .iter()
        .map(|b| {
            b.out
                .items
                .iter()
                .filter_map(|it| if let Item::Label { net, at, rot, .. } = it { Some((*net, *at, *rot)) } else { None })
                .collect()
        })
        .collect();
    let shared = |a: usize, b: usize| labels[a].iter().filter(|x| labels[b].iter().any(|y| y.0 == x.0)).count();
    let mut delta: Vec<Option<Pt>> = vec![None; n];
    let mut rects: Vec<Rect> = Vec::new();
    // Groups go wherever their partners are; the whole arrangement must fit the page at the end.
    let fits = |r: &Rect, rects: &[Rect]| !rects.iter().any(|o| r.inflate(GAP).overlaps(o));

    // Start with the biggest group in the top-left corner.
    let first = (0..n).max_by_key(|&i| (boxes[i].total.w() * boxes[i].total.h(), std::cmp::Reverse(i)))?;
    let d0 = pt(-boxes[first].total.x0, -boxes[first].total.y0);
    if !fits(&boxes[first].total.translate(d0), &rects) {
        return None;
    }
    rects.push(boxes[first].total.translate(d0));
    delta[first] = Some(d0);

    for _ in 1..n {
        let next = (0..n)
            .filter(|&i| delta[i].is_none())
            .max_by_key(|&i| ((0..n).filter(|&j| delta[j].is_some()).map(|j| shared(i, j)).sum::<usize>(), std::cmp::Reverse(i)))?;
        let partner = (0..n).filter(|&j| delta[j].is_some()).max_by_key(|&j| (shared(next, j), std::cmp::Reverse(j))).unwrap();
        let total = boxes[next].total;
        let desired = match labels[next].iter().find_map(|x| labels[partner].iter().find(|y| y.0 == x.0).map(|y| (x, y))) {
            Some((mine, theirs)) if shared(next, partner) > 0 => {
                let p = theirs.1 + delta[partner].unwrap();
                p + dir_vec(theirs.2) * PARTNER - mine.1
            }
            _ => {
                let all = rects.iter().copied().reduce(Rect::union).unwrap();
                pt(all.x1 + GAP - total.x0, -total.y0)
            }
        };
        let desired = pt(snap(desired.x), snap(desired.y));
        let mut found = None;
        'search: for r in (0..=12000i32).step_by(100) {
            for dx in (-r..=r).step_by(100).map(|v: i32| v) {
                for dy in [r - dx.abs(), -(r - dx.abs())] {
                    let d = desired + pt(dx, dy);
                    if fits(&total.translate(d), &rects) {
                        found = Some(d);
                        break 'search;
                    }
                }
            }
        }
        let d = found?;
        rects.push(total.translate(d));
        delta[next] = Some(d);
    }
    let all = rects.iter().copied().reduce(Rect::union)?;
    if all.w() > w || all.h() > h {
        return None;
    }
    let shift = pt(-all.x0, -all.y0);
    Some((0..n).map(|i| pt(boxes[i].total.x0, boxes[i].total.y0) + delta[i].unwrap() + shift).collect())
}

fn usable(p: Paper) -> (i32, i32) {
    let (w, h) = p.size();
    (w - 2 * MARGIN, h - 2 * MARGIN - TITLE_BLOCK)
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
        let sizes: Vec<(i32, i32)> = boxes.iter().map(|b| (b.total.w(), b.total.h())).collect();
        let fit = [Paper::A4, Paper::A3, Paper::A2].into_iter().find_map(|p| {
            let (w, h) = usable(p);
            connected(&boxes, w, h).or_else(|| shelf(&sizes, w, h)).map(|pos| (p, pos))
        });
        let pages: Vec<(Paper, Vec<(Boxed, Pt)>)> = match fit {
            Some((paper, pos)) => vec![(paper, boxes.into_iter().zip(pos).collect())],
            None => {
                // Too much for one A2 sheet: continue on more A3 sheets.
                let (w, h) = usable(Paper::A3);
                let mut pages: Vec<(Paper, Vec<(Boxed, Pt)>)> = Vec::new();
                let mut cur: Vec<Boxed> = Vec::new();
                for b in boxes {
                    let mut trial: Vec<(i32, i32)> = cur.iter().map(|b| (b.total.w(), b.total.h())).collect();
                    trial.push((b.total.w(), b.total.h()));
                    if shelf(&trial, w, h).is_none() && !cur.is_empty() {
                        let sz: Vec<(i32, i32)> = cur.iter().map(|b| (b.total.w(), b.total.h())).collect();
                        let pos = shelf(&sz, w, h).unwrap_or_else(|| vec![pt(0, 0); sz.len()]);
                        pages.push((Paper::A3, std::mem::take(&mut cur).into_iter().zip(pos).collect()));
                    }
                    cur.push(b);
                }
                if !cur.is_empty() {
                    let sz: Vec<(i32, i32)> = cur.iter().map(|b| (b.total.w(), b.total.h())).collect();
                    let (paper, pos) = match shelf(&sz, w, h) {
                        Some(pos) => (Paper::A3, pos),
                        None => {
                            diags.push(Diagnostic::warning("sheet_overflow", format!("a group on sheet `{name}` is larger than an A2 page")));
                            let (w2, _) = usable(Paper::A2);
                            (Paper::A2, shelf(&sz, w2, i32::MAX).unwrap_or_else(|| vec![pt(0, 0); sz.len()]))
                        }
                    };
                    pages.push((paper, cur.into_iter().zip(pos).collect()));
                }
                pages
            }
        };
        let count = pages.len();
        for (pi, (paper, placed)) in pages.into_iter().enumerate() {
            let mut items = Vec::new();
            for (b, pos) in placed {
                let target = pt(MARGIN, MARGIN) + pos;
                let delta = pt(snap(target.x - b.total.x0), snap(target.y - b.total.y0));
                for mut it in b.out.items {
                    it.translate(delta);
                    items.push(it);
                }
                if let Some(f) = b.frame {
                    items.push(Item::Block { name: b.out.name.clone(), rect: f.translate(delta) });
                }
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
