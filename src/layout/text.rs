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
fn part_spots(symbol: &str, orient: Orient, body: Rect, ext: Rect) -> Vec<Spot> {
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
        _ if body.w() <= body.h() => {
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
    let order: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i], Item::Symbol { inst: Some(_), .. }))
        .chain((0..items.len()).filter(|&i| matches!(items[i], Item::Symbol { inst: None, .. })))
        .collect();
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
            let spots = part_spots(symbol, *orient, body, ext);
            let chosen = spots.iter().find(|s| {
                clear(text_rect(at + s[0].0, &fields[ri].value, s[0].1), idx, &ink) && clear(text_rect(at + s[1].0, &fields[vi].value, s[1].1), idx, &ink)
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
