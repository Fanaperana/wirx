//! Wires between neighbouring blocks. A small part (a crystal's resistor, a reset button) that
//! reaches the next block only through a label hides what it is wired to; once the blocks are
//! packed, such a label becomes a short wire to the same net's label in the neighbouring block,
//! wherever that wire fits without crossing or touching anything else.

use super::group::{add_junctions, label_rect};
use super::route::Grid;
use super::Item;
use crate::geom::{Pt, Rect, Rot, GRID};
use crate::ir::Design;
use crate::symbol::{place_pin, SymbolDef};
use std::collections::{HashMap, HashSet};

/// Longest wire drawn between two blocks (mils); further apart, the labels read better.
const MAX_LENGTH: i32 = 3000;
/// At most this many corners.
const MAX_BENDS: usize = 3;
/// Parts with at most this many pins are small (as in eleceda's readability check).
const SMALL_PART_PINS: usize = 4;

/// One way to make a join: which labels go, whether A's label turns to lie along the new wire,
/// and the cells the wire may end on.
struct Attempt {
    drop_a: bool,
    drop_b: bool,
    turn: Option<Rot>,
    targets: HashSet<usize>,
}

struct Label {
    idx: usize,
    net: usize,
    at: Pt,
    rot: Rot,
    frame: Option<usize>,
    /// The stub from the pin to the label: (pin tip, label point).
    stub: Option<(Pt, Pt)>,
    /// The label sits on a pin of a small part (not a connector).
    small: bool,
}

/// Turn label pairs into wires on one page. `counts` holds how many labels each net has in the
/// whole design; a label left alone by a join (no other label of its net anywhere) goes too.
pub fn join_labels(d: &Design, items: &mut Vec<Item>, extra: &[SymbolDef], counts: &mut HashMap<usize, usize>) {
    let def_of = |lib_id: &str, inst: Option<usize>| -> Option<&SymbolDef> {
        match inst {
            Some(i) => Some(&d.symbols[d.instances[i].part]),
            None => extra.iter().find(|s| s.id == lib_id),
        }
    };
    let frames: Vec<Rect> = items.iter().filter_map(|it| if let Item::Block { rect, .. } = it { Some(*rect) } else { None }).collect();
    let pin_owner = pin_owners(d, items);
    let wires: Vec<(Pt, Pt)> = items.iter().filter_map(|it| if let Item::Wire { a, b } = it { Some((*a, *b)) } else { None }).collect();
    let mut labels: Vec<Label> = Vec::new();
    for (idx, it) in items.iter().enumerate() {
        let Item::Label { net, at, rot, .. } = it else { continue };
        let pin = label_pin(*at, &wires, &pin_owner);
        let stub = pin.filter(|p| p != at).map(|p| (p, *at));
        let small = pin.and_then(|p| pin_owner.get(&p)).is_some_and(|o| o.1);
        let frame = frames.iter().position(|f| f.contains(*at));
        labels.push(Label { idx, net: *net, at: *at, rot: *rot, frame, stub, small });
    }
    if !labels.iter().any(|l| l.small) {
        return;
    }

    // Everything already drawn is an obstacle.
    let mut bbox = frames.iter().copied().reduce(Rect::union).unwrap_or(Rect::new(0, 0, 100, 100));
    for &(a, b) in &wires {
        bbox = bbox.union(Rect::from_points(a, b));
    }
    let mut grid = Grid::new(bbox.inflate(400));
    for it in items.iter() {
        match it {
            Item::Wire { a, b } => {
                grid.block_rect(Rect::from_points(*a, *b));
                grid.soft_rect(Rect::from_points(*a, *b).inflate(GRID), 40);
            }
            Item::Junction(p) | Item::NoConnect(p) => grid.block_rect(Rect::point(*p).inflate(GRID)),
            Item::Symbol { lib_id, at, orient, inst, .. } => {
                if let Some(def) = def_of(lib_id, *inst) {
                    grid.block_rect(orient.rect(def.extent()).translate(*at).inflate(GRID));
                }
            }
            Item::Label { .. } | Item::Block { .. } => {}
        }
    }
    for r in super::text::field_rects(items) {
        grid.block_rect(r.inflate(10));
    }
    for l in &labels {
        let Item::Label { text, .. } = &items[l.idx] else { continue };
        grid.block_rect(label_rect(l.at, l.rot, text));
    }

    // Candidate joins: a small part's label to a label of the same net in another block, nearest first.
    let mut pairs: Vec<(i32, usize, usize)> = Vec::new();
    for (a, la) in labels.iter().enumerate() {
        if !la.small || la.frame.is_none() {
            continue;
        }
        for (b, lb) in labels.iter().enumerate() {
            if a != b && lb.net == la.net && lb.frame.is_some() && lb.frame != la.frame {
                pairs.push((la.at.manhattan(lb.at), a, b));
            }
        }
    }
    pairs.sort();
    let mut gone: HashSet<usize> = HashSet::new();
    let mut joined: HashSet<usize> = HashSet::new();
    let mut new_wires: Vec<(Pt, Pt)> = Vec::new();
    let mut turned: HashMap<usize, Rot> = HashMap::new();
    for (dist, a, b) in pairs {
        if dist > MAX_LENGTH || gone.contains(&a) || gone.contains(&b) || joined.contains(&a) || joined.contains(&b) {
            continue;
        }
        let (la, lb) = (&labels[a], &labels[b]);
        let Item::Label { text: ta, .. } = &items[la.idx] else { continue };
        let free = label_rect(la.at, la.rot, ta);
        // When B would be the last label of its net, the wire continues A's stub straight into B's
        // and both labels go. Otherwise it continues A's stub onto B's (a T), and A's label goes; where
        // B's stub has no room for a T (pins 100 mil apart), it ends at B's point, B's label goes and
        // A's label stays, turned to lie along the side of the new wire.
        let last = counts.get(&la.net).copied().unwrap_or(2) <= 2;
        let Item::Label { text: tb, .. } = &items[lb.idx] else { continue };
        let free_b = label_rect(lb.at, lb.rot, tb);
        let at_b: HashSet<usize> = grid.idx(lb.at).into_iter().collect();
        let mut attempts: Vec<Attempt> = Vec::new();
        if last {
            attempts.push(Attempt { drop_a: true, drop_b: true, turn: None, targets: at_b.clone() });
        } else {
            if let Some((tip, end)) = lb.stub {
                let on_stub: HashSet<usize> = grid.segment_cells(tip, end).into_iter().filter(|&c| grid.pt(c) != tip).collect();
                attempts.push(Attempt { drop_a: true, drop_b: false, turn: None, targets: on_stub });
            }
            for turn in [90, 270] {
                attempts.push(Attempt { drop_a: false, drop_b: true, turn: Some(crate::geom::norm_rot(la.rot as i32 + turn)), targets: at_b.clone() });
            }
        }
        let mut found: Option<(bool, bool, Option<Rot>, Vec<Pt>)> = None;
        for Attempt { drop_a, drop_b, turn, targets } in attempts {
            let mut g = grid.clone();
            // Only through the two blocks being joined and the space between blocks.
            for (k, f) in frames.iter().enumerate() {
                if Some(k) != la.frame && Some(k) != lb.frame {
                    g.block_rect(*f);
                }
            }
            if drop_a || turn.is_some() {
                unblock(&mut g, free, &wires, &new_wires);
            }
            if drop_b {
                unblock(&mut g, free_b, &wires, &new_wires);
            }
            if let Some(r) = turn {
                // A's label turns to lie along the wire's side; its space must be free.
                let moved = label_rect(la.at, r, ta).inflate(-10);
                if !g.rect_free(moved) {
                    continue;
                }
                g.block_rect(label_rect(la.at, r, ta));
                g.unblock_rect(Rect::from_points(la.at, la.at + crate::geom::dir_vec(la.rot) * 200));
            }
            let best = g.route(-1, la.at, la.rot, &targets).filter(|p| p.len() <= MAX_BENDS + 2 && p.windows(2).map(|w| w[0].manhattan(w[1])).sum::<i32>() <= MAX_LENGTH);
            if let Some(p) = best {
                found = Some((drop_a, drop_b, turn, p));
                break;
            }
        }
        let Some((drop_a, drop_b, turn, path)) = found else { continue };
        for w in path.windows(2) {
            new_wires.push((w[0], w[1]));
            grid.block_rect(Rect::from_points(w[0], w[1]));
            grid.soft_rect(Rect::from_points(w[0], w[1]).inflate(GRID), 40);
        }
        for (dropped, k, r) in [(drop_a, a, free), (drop_b, b, free_b)] {
            if dropped {
                unblock(&mut grid, r, &wires, &new_wires);
                gone.insert(k);
                *counts.entry(la.net).or_insert(1) -= 1;
            }
        }
        if let Some(r) = turn {
            unblock(&mut grid, free, &wires, &new_wires);
            grid.block_rect(label_rect(la.at, r, ta));
            turned.insert(la.idx, r);
        }
        joined.extend([a, b]);
    }
    if gone.is_empty() && turned.is_empty() {
        return;
    }
    for (&idx, &r) in &turned {
        if let Item::Label { rot, .. } = &mut items[idx] {
            *rot = r;
        }
    }
    let drop: HashSet<usize> = gone.iter().map(|&k| labels[k].idx).collect();
    let mut k = 0;
    items.retain(|_| {
        let keep = !drop.contains(&k);
        k += 1;
        keep
    });
    items.retain(|it| !matches!(it, Item::Junction(_)));
    items.extend(new_wires.into_iter().map(|(a, b)| Item::Wire { a, b }));
    add_junctions(d, items);
}

/// Free the space of a label that is being replaced, keeping the wires that pass through it.
fn unblock(g: &mut Grid, r: Rect, wires: &[(Pt, Pt)], more: &[(Pt, Pt)]) {
    g.unblock_rect(r);
    for &(a, b) in wires.iter().chain(more) {
        let w = Rect::from_points(a, b);
        if w.intersects(&r) {
            g.block_rect(w);
        }
    }
}

/// Nets whose label in `items` sits on a pin of a small part.
pub fn small_label_nets(d: &Design, items: &[Item]) -> HashSet<usize> {
    let owners = pin_owners(d, items);
    let wires: Vec<(Pt, Pt)> = items.iter().filter_map(|it| if let Item::Wire { a, b } = it { Some((*a, *b)) } else { None }).collect();
    items
        .iter()
        .filter_map(|it| match it {
            Item::Label { net, at, .. } => label_pin(*at, &wires, &owners).and_then(|p| owners.get(&p)).filter(|o| o.1).map(|_| *net),
            _ => None,
        })
        .collect()
}

/// Pin tips of parts: (instance, whether it is a small part).
fn pin_owners(d: &Design, items: &[Item]) -> HashMap<Pt, (usize, bool)> {
    let mut out = HashMap::new();
    for it in items {
        if let Item::Symbol { at, orient, inst: Some(i), .. } = it {
            let sym = &d.symbols[d.instances[*i].part];
            let r = &d.instances[*i].reference;
            let connector = ["J", "P", "TP", "CN"].iter().any(|p| r.strip_prefix(p).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())));
            let small = (2..=SMALL_PART_PINS).contains(&sym.pins.len()) && !connector;
            for p in &sym.pins {
                out.insert(place_pin(p, *orient, *at).at, (*i, small));
            }
        }
    }
    out
}

/// The pin a label at `at` belongs to: at the label itself, or at the other end of its stub.
fn label_pin(at: Pt, wires: &[(Pt, Pt)], owners: &HashMap<Pt, (usize, bool)>) -> Option<Pt> {
    if owners.contains_key(&at) {
        return Some(at);
    }
    wires.iter().find_map(|&(a, b)| if b == at && owners.contains_key(&a) { Some(a) } else if a == at && owners.contains_key(&b) { Some(b) } else { None })
}
