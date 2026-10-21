//! Rectangular IC symbols generated from a pin list (a port of eleceda's `boxSymbol`).
//!
//! Without explicit `@side`, pins go where readers expect them: inputs on the left, outputs on the
//! right, supplies on top and grounds at the bottom. Remaining pins are split between left and
//! right in declaration order so both sides stay balanced.

use super::templates::{rect, std_fields};
use super::{Fill, Pin, PinType, SymbolDef};
use crate::geom::{pt, Pt, Rot};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "left" | "l" => Some(Side::Left),
            "right" | "r" => Some(Side::Right),
            "top" | "t" | "up" => Some(Side::Top),
            "bottom" | "b" | "down" => Some(Side::Bottom),
            _ => None,
        }
    }
}

pub struct BoxPin {
    pub number: String,
    pub name: String,
    pub ptype: PinType,
    pub side: Option<Side>,
    pub hidden: bool,
}

pub fn is_ground_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("GND") || n.ends_with("GND") || n == "VSS" || n.starts_with("VSS") || n == "VEE" || n == "EP" || n == "PAD"
}

pub fn auto_side(p: &BoxPin) -> Option<Side> {
    match p.ptype {
        PinType::PowerIn | PinType::PowerOut if is_ground_name(&p.name) => Some(Side::Bottom),
        PinType::PowerIn => Some(Side::Top),
        PinType::PowerOut | PinType::Output | PinType::OpenCollector | PinType::OpenEmitter | PinType::TriState => Some(Side::Right),
        PinType::Input => Some(Side::Left),
        _ => None,
    }
}

pub fn box_symbol(id: &str, name: &str, prefix: &str, pins: &[BoxPin]) -> SymbolDef {
    let pitch = 100;
    let len = 100;
    let mut sides: [Vec<(usize, &BoxPin)>; 4] = [vec![], vec![], vec![], vec![]];
    let idx = |s: Side| match s {
        Side::Left => 0,
        Side::Right => 1,
        Side::Top => 2,
        Side::Bottom => 3,
    };
    // A hidden pin named like a visible one (the other DVDD or GND pins of a chip) stacks behind
    // it, the way chip symbols draw them, instead of taking a place of its own on the edge.
    let stack_on: HashMap<usize, usize> = pins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.hidden)
        .filter_map(|(i, p)| {
            let side = p.side.or_else(|| auto_side(p));
            let host = pins.iter().position(|q| !q.hidden && q.name == p.name && q.ptype == p.ptype && (p.side.is_none() || q.side.or_else(|| auto_side(q)) == side))?;
            Some((i, host))
        })
        .collect();
    let mut flexible = Vec::new();
    for (i, p) in pins.iter().enumerate() {
        if stack_on.contains_key(&i) {
            continue;
        }
        match p.side.or_else(|| auto_side(p)) {
            Some(s) => sides[idx(s)].push((i, p)),
            None => flexible.push((i, p)),
        }
    }
    // Balance flexible pins: fill the shorter side first, keeping declaration order on each side.
    if !flexible.is_empty() {
        let total = sides[0].len() + sides[1].len() + flexible.len();
        let left_target = total.div_ceil(2);
        let to_left = left_target.saturating_sub(sides[0].len()).min(flexible.len());
        for (i, p) in flexible.into_iter().enumerate() {
            if i < to_left { sides[0].push(p) } else { sides[1].push(p) }
        }
    }

    let char_w = 35;
    let max_name = |list: &Vec<(usize, &BoxPin)>| list.iter().map(|b| b.1.name.chars().count() as i32).max().unwrap_or(0);
    let v_count = sides[0].len().max(sides[1].len()).max(1) as i32;
    let h_count = sides[2].len().max(sides[3].len()) as i32;
    let snap_up = |v: i32| (v + 99).div_euclid(100) * 100;

    let name_w = (max_name(&sides[0]) + max_name(&sides[1])) * char_w + 150;
    let half_w = snap_up((name_w / 2).max(h_count * pitch / 2 + 50).max(200));
    let name_h = (max_name(&sides[2]) + max_name(&sides[3])) * char_w + 100;
    let half_h = snap_up((((v_count - 1) * pitch) / 2 + 100).max(name_h / 2).max(100));

    let start = |n: usize| -((((n as i32 - 1) * pitch) / 2) / 50) * 50;
    let mut out: Vec<(usize, Pin)> = Vec::new();
    let mut place = |list: &Vec<(usize, &BoxPin)>, at: &dyn Fn(i32) -> Pt, dir: Rot| {
        for (i, &(decl, b)) in list.iter().enumerate() {
            out.push((decl, Pin {
                at: at(i as i32),
                dir,
                length: len,
                number: b.number.clone(),
                name: b.name.clone(),
                ptype: b.ptype,
                shape: "line".into(),
                unit: 0,
                hidden: b.hidden.then_some(true),
            }));
        }
    };
    let (l, r, t, b) = (start(sides[0].len()), start(sides[1].len()), start(sides[2].len()), start(sides[3].len()));
    place(&sides[0], &|i| pt(-half_w - len, l + i * pitch), 0);
    place(&sides[1], &|i| pt(half_w + len, r + i * pitch), 180);
    place(&sides[2], &|i| pt(t + i * pitch, -half_h - len), 270);
    place(&sides[3], &|i| pt(b + i * pitch, half_h + len), 90);
    for (&i, &host) in &stack_on {
        let h = out.iter().find(|p| p.0 == host).unwrap().1.clone();
        let p = &pins[i];
        out.push((i, Pin { number: p.number.clone(), name: p.name.clone(), ptype: p.ptype, hidden: Some(true), ..h }));
    }
    // Keep the part's declaration order: pin k of the symbol is pin k of the part.
    out.sort_by_key(|p| p.0);
    let out: Vec<Pin> = out.into_iter().map(|p| p.1).collect();

    SymbolDef {
        id: id.into(),
        name: name.into(),
        description: None,
        keywords: None,
        ref_prefix: prefix.into(),
        unit_count: 1,
        power: None,
        hide_pin_names: None,
        hide_pin_numbers: None,
        pin_name_offset: Some(20),
        open_pins_ok: None,
        graphics: vec![rect(-half_w, -half_h, half_w, half_h, Some(10), Some(Fill::Background))],
        pins: out,
        fields: std_fields(prefix, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bp(n: &str, name: &str, t: PinType) -> BoxPin {
        BoxPin { number: n.into(), name: name.into(), ptype: t, side: None, hidden: false }
    }

    #[test]
    fn sides_follow_pin_types() {
        let s = box_symbol(
            "wirx:X",
            "X",
            "U",
            &[bp("1", "GND", PinType::PowerIn), bp("2", "OUT", PinType::Output), bp("3", "IN", PinType::Input), bp("4", "VCC", PinType::PowerIn)],
        );
        let pin = |n: &str| s.pins.iter().find(|p| p.number == n).unwrap();
        assert_eq!(pin("1").dir, 90); // bottom, pointing up into the body
        assert_eq!(pin("2").dir, 180); // right
        assert_eq!(pin("3").dir, 0); // left
        assert_eq!(pin("4").dir, 270); // top
        for p in &s.pins {
            assert_eq!(p.at.x % 50, 0);
            assert_eq!(p.at.y % 50, 0);
        }
    }
}

/// One place along a side of a laid-out symbol.
#[derive(Debug, Clone)]
pub enum Slot {
    /// Blank space separating pin groups.
    Gap,
    /// A pin; several numbers are stacked (drawn once, the others hidden behind it).
    Pin { numbers: Vec<String>, name: String, ptype: PinType },
}

/// A rectangular symbol with pins exactly where the author put them: per side, in order, with
/// gaps between groups and stacked pins (the way IC symbols are drawn by hand). `min_width` and
/// `min_height` widen the body.
///
/// Pins come out side by side (left, right, top, bottom), each slot's numbers in order: that is
/// the order of the part's pins.
pub fn layout_symbol(id: &str, name: &str, prefix: &str, sides: &[Vec<Slot>; 4], min_width: i32, min_height: i32) -> SymbolDef {
    let pitch = 100;
    let len = 100;
    let char_w = 35;
    let longest = |list: &Vec<Slot>| {
        list.iter()
            .map(|s| match s {
                Slot::Pin { name, .. } => name.chars().count() as i32,
                Slot::Gap => 0,
            })
            .max()
            .unwrap_or(0)
    };
    let count = |list: &Vec<Slot>| list.len() as i32;
    let snap_up = |v: i32| (v + 99).div_euclid(100) * 100;
    let v_count = count(&sides[0]).max(count(&sides[1])).max(1);
    let h_count = count(&sides[2]).max(count(&sides[3]));
    let name_w = (longest(&sides[0]) + longest(&sides[1])) * char_w + 150;
    let half_w = snap_up((name_w / 2).max(h_count * pitch / 2 + 50).max(200).max(min_width / 2));
    let name_h = (longest(&sides[2]) + longest(&sides[3])) * char_w + 100;
    let half_h = snap_up((((v_count - 1) * pitch) / 2 + 100).max(name_h / 2).max(100).max(min_height / 2));
    let start = |n: i32| -((((n - 1).max(0) * pitch) / 2) / 50) * 50;

    let mut pins = Vec::new();
    let place = |list: &Vec<Slot>, at: &dyn Fn(i32) -> Pt, dir: Rot, pins: &mut Vec<Pin>| {
        for (i, slot) in list.iter().enumerate() {
            if let Slot::Pin { numbers, name, ptype } = slot {
                for (k, number) in numbers.iter().enumerate() {
                    pins.push(Pin {
                        at: at(i as i32),
                        dir,
                        length: len,
                        number: number.clone(),
                        name: name.clone(),
                        ptype: *ptype,
                        shape: "line".into(),
                        unit: 0,
                        hidden: (k > 0).then_some(true),
                    });
                }
            }
        }
    };
    let (l, r, t, b) = (start(count(&sides[0])), start(count(&sides[1])), start(count(&sides[2])), start(count(&sides[3])));
    place(&sides[0], &|i| pt(-half_w - len, l + i * pitch), 0, &mut pins);
    place(&sides[1], &|i| pt(half_w + len, r + i * pitch), 180, &mut pins);
    place(&sides[2], &|i| pt(t + i * pitch, -half_h - len), 270, &mut pins);
    place(&sides[3], &|i| pt(b + i * pitch, half_h + len), 90, &mut pins);

    SymbolDef {
        id: id.into(),
        name: name.into(),
        description: None,
        keywords: None,
        ref_prefix: prefix.into(),
        unit_count: 1,
        power: None,
        hide_pin_names: None,
        hide_pin_numbers: None,
        pin_name_offset: Some(20),
        open_pins_ok: None,
        graphics: vec![rect(-half_w, -half_h, half_w, half_h, Some(10), Some(Fill::Background))],
        pins,
        fields: std_fields(prefix, name),
    }
}
