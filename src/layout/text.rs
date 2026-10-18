//! Final text placement: once wires exist, move each reference/value (and power symbol name) to
//! the first nearby spot where it covers no wire, label, part or other text. Text carries no
//! connectivity, so this never changes the circuit.

use super::Item;
use crate::geom::{pt, text_width, Orient, Pt, Rect};
use crate::ir::Design;
use crate::symbol::{Justify, SymbolDef};

const H: i32 = 30;

fn text_rect(at: Pt, value: &str, j: Justify) -> Rect {
    let w = text_width(value, 50);
    let x = match j {
        Justify::Left => at.x,
        Justify::Right => at.x - w,
        Justify::Center => at.x - w / 2,
    };
    Rect::new(x, at.y - H, x + w, at.y + H)
}

type Spot = [(Pt, Justify); 2];

/// Candidate (reference, value) positions relative to the symbol origin, best first.
/// `upright`: a two-pin part standing with its pins up and down (text beside it reads as its
/// own; above and below it would sit beyond its wires and power symbols).
fn part_spots(symbol: &str, orient: Orient, body: Rect, ext: Rect, upright: bool) -> Vec<Spot> {
    use Justify::*;
    let cy = body.center().y;
    let cx = body.center().x;
    let mut v: Vec<Spot> = Vec::new();
    let right = |dy: i32| [(pt(body.x1 + 40, cy - 30 + dy), Left), (pt(body.x1 + 40, cy + 30 + dy), Left)];
    let left = |dy: i32| [(pt(body.x0 - 40, cy - 30 + dy), Right), (pt(body.x0 - 40, cy + 30 + dy), Right)];
    let split = |x: i32, j: Justify, k: i32| [(pt(x, ext.y0 - 50 - 60 * k), j), (pt(x, ext.y1 + 50 + 60 * k), j)];
    let above = |x: i32, j: Justify, k: i32| [(pt(x, ext.y0 - 110 - 60 * k), j), (pt(x, ext.y0 - 50 - 60 * k), j)];
    let below = |x: i32, j: Justify, k: i32| [(pt(x, ext.y1 + 50 + 60 * k), j), (pt(x, ext.y1 + 110 + 60 * k), j)];
    let shifts = [0, -60, 60, -120, 120, -180, 180];
    match symbol {
        "box" | "conn" | "conn2" => {
            for k in 0..8 {
                v.push(split(body.x0, Left, k));
                v.push(split(body.x1, Right, k));
            }
            for k in 0..8 {
                v.push(above(body.x0, Left, k));
                v.push(below(body.x0, Left, k));
            }
            for dy in shifts {
                v.push([(pt(ext.x1 + 40, ext.y0 + 30 + dy), Left), (pt(ext.x1 + 40, ext.y0 + 90 + dy), Left)]);
                v.push([(pt(ext.x0 - 40, ext.y0 + 30 + dy), Right), (pt(ext.x0 - 40, ext.y0 + 90 + dy), Right)]);
            }
        }
        "npn" | "pnp" | "nmos" | "pmos" => {
            let (first, second): (&dyn Fn(i32) -> Spot, &dyn Fn(i32) -> Spot) = if orient.mirror { (&left, &right) } else { (&right, &left) };
            for dy in shifts {
                v.push(first(dy));
            }
            for dy in shifts {
                v.push(second(dy));
            }
            for k in 0..4 {
                v.push(split(cx, Center, k));
            }
        }
        _ if upright || body.w() <= body.h() && !is_lying(body, ext) => {
            for dy in shifts {
                v.push(right(dy));
                v.push(left(dy));
            }
            for k in 0..4 {
                v.push(split(cx, Center, k));
                v.push(above(cx, Center, k));
                v.push(below(cx, Center, k));
            }
        }
        _ => {
            for k in 0..5 {
                v.push(split(cx, Center, k));
                v.push(above(cx, Center, k));
                v.push(below(cx, Center, k));
            }
            for dy in shifts {
                v.push(right(dy));
                v.push(left(dy));
            }
        }
    }
    v
}

/// Spots close to a chip's body for one text, best first: just above (or below) the body, at a
/// corner or beside the outermost pins of that edge; `above_first` says which edge comes first
/// (the reference goes above, the value below).
fn chip_spots(body: Rect, pins: &[(Pt, crate::geom::Rot)], above_first: bool) -> Vec<(Pt, Justify)> {
    use Justify::*;
    let mut edges = [(90, body.y0 - 50, body.y0 - 110), (270, body.y1 + 50, body.y1 + 110)];
    if !above_first {
        edges.reverse();
    }
    let mut v = Vec::new();
    for (out, y, y2) in edges {
        let xs: Vec<i32> = pins.iter().filter(|p| p.1 == out).map(|p| p.0.x).collect();
        for yy in [y, y2] {
            v.push((pt(body.x0, yy), Left));
            v.push((pt(body.x1, yy), Right));
            if let (Some(&lo), Some(&hi)) = (xs.iter().min(), xs.iter().max()) {
                v.push((pt(lo - 100, yy), Right));
                v.push((pt(hi + 100, yy), Left));
            }
        }
    }
    v
}

/// A part whose pins stick out sideways (wider extent than body, same height).
fn is_lying(body: Rect, ext: Rect) -> bool {
    ext.w() > body.w() && ext.h() <= body.h()
}

fn power_spots(default: Pt, down: bool) -> Vec<(Pt, Justify)> {
    use Justify::*;
    let y = if down { 60 } else { -60 };
    vec![
        (default, Center),
        (pt(70, y), Left),
        (pt(-70, y), Right),
        (pt(0, default.y + if down { 70 } else { -70 }), Center),
        (pt(70, y + if down { 60 } else { -60 }), Left),
        (pt(-70, y + if down { 60 } else { -60 }), Right),
    ]
}

pub fn place(d: &Design, items: &mut [Item], extra: &[SymbolDef]) {
    let original = items.to_vec();
    place_in_order(d, items, extra, false);
    if collisions(d, items, extra) == 0 {
        return;
    }
    // A power name with only a few spots can be boxed in by part text placed first: then try
    // the power names first, and keep whichever leaves less covered.
    let mut other = original;
    place_in_order(d, &mut other, extra, true);
    if collisions(d, &other, extra) < collisions(d, items, extra) {
        items.clone_from_slice(&other);
    }
}

fn place_in_order(d: &Design, items: &mut [Item], extra: &[SymbolDef], power_first: bool) {
    let def_of = |lib_id: &str, inst: Option<usize>| -> Option<SymbolDef> {
        match inst {
            Some(i) => Some(d.symbols[d.instances[i].part].clone()),
            None => extra.iter().find(|s| s.id == lib_id).cloned(),
        }
    };
    // Ink that text must not cover, tagged with the owning item (text may touch its own body).
    let mut ink: Vec<(Rect, usize)> = Vec::new();
    const NONE: usize = usize::MAX;
    for (idx, it) in items.iter().enumerate() {
        match it {
            Item::Wire { a, b } => ink.push((Rect::from_points(*a, *b).inflate(10), NONE)),
            Item::Junction(p) | Item::NoConnect(p) => ink.push((Rect::point(*p).inflate(30), NONE)),
            Item::Label { text, at, rot, .. } => ink.push((super::group::label_rect(*at, *rot, text), NONE)),
            Item::Symbol { lib_id, at, orient, inst, .. } => {
                if let Some(def) = def_of(lib_id, *inst) {
                    let local = if def.is_power() { def.extent() } else { def.body() };
                    ink.push((orient.rect(local).translate(*at), idx));
                    // Pin lines: no text on them, the part's own included.
                    if !def.is_power() {
                        for p in &def.pins {
                            let tip = orient.apply(p.at) + *at;
                            let end = tip + crate::geom::dir_vec(orient.angle(p.dir)) * p.length;
                            ink.push((Rect::from_points(tip, end).inflate(10), NONE));
                        }
                    }
                }
            }
            Item::Block { .. } => {}
        }
    }
    let clear = |r: Rect, own: usize, ink: &[(Rect, usize)]| {
        let r = r.inflate(-5);
        !ink.iter().any(|(o, who)| *who != own && o.intersects(&r))
    };

    // Parts first (they have the most freedom to keep text close), then power symbol names.
    let parts = (0..items.len()).filter(|&i| matches!(items[i], Item::Symbol { inst: Some(_), .. }));
    let power = (0..items.len()).filter(|&i| matches!(items[i], Item::Symbol { inst: None, .. }));
    let order: Vec<usize> = if power_first { power.chain(parts).collect() } else { parts.chain(power).collect() };
    for idx in order {
        let Item::Symbol { lib_id, at, orient, inst, fields, .. } = &mut items[idx] else { continue };
        let Some(def) = def_of(lib_id, *inst) else { continue };
        let at = *at;
        if inst.is_some() {
            let symbol = &d.parts[d.instances[inst.unwrap()].part].symbol;
            let body = orient.rect(def.body());
            let ext = orient.rect(def.extent());
            let (ri, vi) = (fields.iter().position(|f| f.name == "Reference"), fields.iter().position(|f| f.name == "Value"));
            let (Some(ri), Some(vi)) = (ri, vi) else { continue };
            let upright = def.pins.len() == 2 && def.pins.iter().all(|p| !crate::geom::is_horizontal(crate::symbol::place_pin(p, *orient, pt(0, 0)).out));
            let spots = part_spots(symbol, *orient, body, ext, upright);
            // A chip's reference and value each take the closest clear spot by its body.
            if matches!(symbol.as_str(), "box" | "conn" | "conn2" | "custom") {
                let pins: Vec<(Pt, crate::geom::Rot)> = def.pins.iter().map(|p| (orient.apply(p.at), crate::geom::opposite(orient.angle(p.dir)))).collect();
                let pick = |fi: usize, above: bool, taken: Option<Rect>, ink: &[(Rect, usize)]| {
                    chip_spots(body, &pins, above).into_iter().find(|s| {
                        let r = text_rect(at + s.0, &fields[fi].value, s.1);
                        clear(r, idx, ink) && taken.is_none_or(|t| !t.inflate(-5).intersects(&r.inflate(-5)))
                    })
                };
                if let Some(r) = pick(ri, true, None, &ink) {
                    let r_rect = text_rect(at + r.0, &fields[ri].value, r.1);
                    if let Some(v) = pick(vi, false, Some(r_rect), &ink) {
                        (fields[ri].at, fields[ri].justify) = (r.0, Some(r.1));
                        (fields[vi].at, fields[vi].justify) = (v.0, Some(v.1));
                        for f in [&fields[ri], &fields[vi]] {
                            ink.push((text_rect(at + f.at, &f.value, f.justify.unwrap_or(Justify::Center)), idx));
                        }
                        continue;
                    }
                }
            }
            let chosen = spots.iter().find(|s| {
                let (a, b) = (text_rect(at + s[0].0, &fields[ri].value, s[0].1), text_rect(at + s[1].0, &fields[vi].value, s[1].1));
                clear(a, idx, &ink) && clear(b, idx, &ink) && !a.inflate(-5).intersects(&b.inflate(-5))
            });
            if let Some(s) = chosen {
                fields[ri].at = s[0].0;
                fields[ri].justify = Some(s[0].1);
                fields[vi].at = s[1].0;
                fields[vi].justify = Some(s[1].1);
            }
            for f in [&fields[ri], &fields[vi]] {
                ink.push((text_rect(at + f.at, &f.value, f.justify.unwrap_or(Justify::Center)), idx));
            }
        } else {
            let Some(vi) = fields.iter().position(|f| f.name == "Value" && f.visible) else { continue };
            let down = fields[vi].at.y > 0;
            let spots = power_spots(fields[vi].at, down);
            if let Some(s) = spots.iter().find(|s| clear(text_rect(at + s.0, &fields[vi].value, s.1), idx, &ink)) {
                fields[vi].at = s.0;
                fields[vi].justify = Some(s.1);
            }
            let f = &fields[vi];
            ink.push((text_rect(at + f.at, &f.value, f.justify.unwrap_or(Justify::Center)), idx));
        }
    }
}

/// Visible texts on a sheet that overlap each other: part references and values, power names
/// and net labels, across groups too. Each pair as (text, text).
pub fn overlaps(items: &[Item]) -> Vec<(String, String)> {
    let mut texts: Vec<(Rect, String)> = Vec::new();
    for it in items {
        match it {
            Item::Symbol { at, fields, .. } => {
                for f in fields.iter().filter(|f| f.visible) {
                    texts.push((text_rect(*at + f.at, &f.value, f.justify.unwrap_or(Justify::Center)), f.value.clone()));
                }
            }
            Item::Label { text, at, rot, .. } => texts.push((super::group::label_rect(*at, *rot, text), text.clone())),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (i, (a, ta)) in texts.iter().enumerate() {
        for (b, tb) in &texts[i + 1..] {
            if a.inflate(-5).intersects(&b.inflate(-5)) {
                out.push((ta.clone(), tb.clone()));
            }
        }
    }
    out
}

/// How far each part's reference and value text stands from the part's body (mils, 0 when it
/// touches it): (reference, field value, gap).
pub fn distances(items: &[Item], defs: &[SymbolDef]) -> Vec<(String, String, i32)> {
    let gap = |a: Rect, b: Rect| (a.x0 - b.x1).max(b.x0 - a.x1).max(a.y0 - b.y1).max(b.y0 - a.y1).max(0);
    let mut out = Vec::new();
    for it in items {
        let Item::Symbol { lib_id, at, orient, fields, inst: Some(_), .. } = it else { continue };
        let Some(def) = defs.iter().find(|d| &d.id == lib_id) else { continue };
        let body = orient.rect(def.body()).translate(*at);
        let reference = fields.iter().find(|f| f.name == "Reference").map(|f| f.value.clone()).unwrap_or_default();
        for f in fields.iter().filter(|f| f.visible && (f.name == "Reference" || f.name == "Value")) {
            let r = text_rect(*at + f.at, &f.value, f.justify.unwrap_or(Justify::Center));
            out.push((reference.clone(), f.value.clone(), gap(r, body)));
        }
    }
    out
}

/// Space the visible text of a symbol item covers.
pub fn field_rects(items: &[Item]) -> Vec<Rect> {
    let mut out = Vec::new();
    for it in items {
        if let Item::Symbol { at, fields, .. } = it {
            for f in fields.iter().filter(|f| f.visible) {
                out.push(text_rect(*at + f.at, &f.value, f.justify.unwrap_or(Justify::Center)));
            }
        }
    }
    out
}

/// Visible text covering anything that is not its own symbol, plus part bodies that overlap:
/// the same rule as eleceda's text-overlap check, used to compare layouts.
pub fn collisions(d: &Design, items: &[Item], extra: &[SymbolDef]) -> usize {
    let def_of = |lib_id: &str, inst: Option<usize>| -> Option<SymbolDef> {
        match inst {
            Some(i) => Some(d.symbols[d.instances[i].part].clone()),
            None => extra.iter().find(|s| s.id == lib_id).cloned(),
        }
    };
    const NONE: usize = usize::MAX;
    let mut ink: Vec<(Rect, usize)> = Vec::new();
    let mut texts: Vec<(Rect, usize)> = Vec::new();
    let mut bodies: Vec<Rect> = Vec::new();
    for (idx, it) in items.iter().enumerate() {
        match it {
            Item::Wire { a, b } => ink.push((Rect::from_points(*a, *b).inflate(10), NONE)),
            Item::Junction(p) | Item::NoConnect(p) => ink.push((Rect::point(*p).inflate(30), NONE)),
            Item::Label { text, at, rot, .. } => ink.push((super::group::label_rect(*at, *rot, text), NONE)),
            Item::Symbol { lib_id, at, orient, inst, fields, .. } => {
                let Some(def) = def_of(lib_id, *inst) else { continue };
                let local = if def.is_power() { def.extent() } else { def.body() };
                let body = orient.rect(local).translate(*at);
                ink.push((body, idx));
                if inst.is_some() {
                    bodies.push(orient.rect(def.body()).translate(*at));
                }
                for f in fields.iter().filter(|f| f.visible && !f.value.is_empty() && (!def.is_power() || f.name == "Value")) {
                    let r = text_rect(*at + f.at, &f.value, f.justify.unwrap_or(Justify::Center));
                    texts.push((r, idx));
                }
            }
            Item::Block { .. } => {}
        }
    }
    let mut count = 0;
    for (i, &(r, own)) in texts.iter().enumerate() {
        let r = r.inflate(-5);
        let hit = ink.iter().any(|(o, who)| *who != own && o.intersects(&r)) || texts.iter().enumerate().any(|(j, (o, who))| j != i && *who != own && o.intersects(&r));
        count += usize::from(hit);
    }
    for a in 0..bodies.len() {
        for b in a + 1..bodies.len() {
            count += usize::from(bodies[a].inflate(-40).overlaps(&bodies[b].inflate(-40)));
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_labels_are_found() {
        let label = |x: i32, text: &str| Item::Label { net: 0, text: text.into(), at: pt(x, 0), rot: 0, global: false, shape: "bidirectional" };
        assert_eq!(overlaps(&[label(0, "SCK"), label(100, "MOSI")]).len(), 1);
        assert!(overlaps(&[label(0, "SCK"), label(1000, "MOSI")]).is_empty());
    }
}
