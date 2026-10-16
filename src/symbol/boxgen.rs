//! Rectangular IC symbols generated from a pin list (a port of eleceda's `boxSymbol`).
//!
//! Without explicit `@side`, pins go where readers expect them: inputs on the left, outputs on the
//! right, supplies on top and grounds at the bottom. Remaining pins are split between left and
//! right in declaration order so both sides stay balanced.

use super::templates::{rect, std_fields};
use super::{Fill, Pin, PinType, SymbolDef};
use crate::geom::{pt, Pt, Rot};

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

fn auto_side(p: &BoxPin) -> Option<Side> {
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
    let mut flexible = Vec::new();
    for (i, p) in pins.iter().enumerate() {
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
