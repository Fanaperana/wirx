//! Hand-drawn symbol shapes (ported from eleceda's built-in library) and power symbols.
//!
//! A template has *slots*: pin positions in a fixed order. A part that uses a template maps its
//! declared pins to the slots in declaration order, so e.g. `symbol npn` expects pins B, C, E.

use super::{Field, Fill, Graphic, Justify, Pin, PinType, RailKind, SymbolDef};
use crate::geom::{pt, Pt, Rot};

pub struct Slot {
    pub at: Pt,
    pub dir: Rot,
    pub length: i32,
}

pub struct Template {
    pub graphics: Vec<Graphic>,
    pub slots: Vec<Slot>,
    pub hide_pin_names: bool,
    pub hide_pin_numbers: bool,
    /// Slot names in order, shown in errors (`B, C, E`).
    pub slot_names: &'static str,
}

fn slot(x: i32, y: i32, dir: Rot, length: i32) -> Slot {
    Slot { at: pt(x, y), dir, length }
}

pub fn poly(points: &[(i32, i32)], width: Option<i32>, fill: Option<Fill>) -> Graphic {
    Graphic::Polyline { points: points.iter().map(|&(x, y)| pt(x, y)).collect(), unit: 0, width, fill: Some(fill.unwrap_or(Fill::None)) }
}
pub fn rect(x0: i32, y0: i32, x1: i32, y1: i32, width: Option<i32>, fill: Option<Fill>) -> Graphic {
    Graphic::Rect { start: pt(x0, y0), end: pt(x1, y1), unit: 0, width, fill: Some(fill.unwrap_or(Fill::None)) }
}
fn circle(x: i32, y: i32, r: i32, width: Option<i32>) -> Graphic {
    Graphic::Circle { center: pt(x, y), radius: r, unit: 0, width, fill: Some(Fill::None) }
}
fn arc(s: (i32, i32), m: (i32, i32), e: (i32, i32), width: Option<i32>) -> Graphic {
    Graphic::Arc { start: pt(s.0, s.1), mid: pt(m.0, m.1), end: pt(e.0, e.1), unit: 0, width, fill: Some(Fill::None) }
}

const W10: Option<i32> = Some(10);

fn vslots(len: i32) -> Vec<Slot> {
    vec![slot(0, -150, 270, len), slot(0, 150, 90, len)]
}

fn diode_body() -> Vec<Graphic> {
    vec![poly(&[(50, 50), (50, -50), (-50, 0), (50, 50)], W10, None), poly(&[(0, 0), (50, 0)], None, None)]
}

fn bjt_body() -> Vec<Graphic> {
    vec![circle(50, 0, 125, W10), poly(&[(0, -65), (0, 65)], Some(20), None)]
}

fn mos_body(arrow_in: bool) -> Vec<Graphic> {
    vec![
        circle(50, 0, 125, W10),
        poly(&[(-40, -60), (-40, 60)], W10, None),
        poly(&[(0, -75), (0, -40)], Some(15), None),
        poly(&[(0, -15), (0, 15)], Some(15), None),
        poly(&[(0, 40), (0, 75)], Some(15), None),
        poly(&[(0, -55), (100, -55), (100, -100)], None, None),
        poly(&[(0, 55), (100, 55), (100, 100)], None, None),
        poly(&[(0, 0), (100, 0), (100, 55)], None, None),
        if arrow_in {
            poly(&[(5, 0), (45, -18), (45, 18), (5, 0)], None, Some(Fill::Outline))
        } else {
            poly(&[(45, 0), (10, -18), (10, 18), (45, 0)], None, Some(Fill::Outline))
        },
    ]
}

pub const TEMPLATE_NAMES: &[&str] = &[
    "resistor", "capacitor", "cap_polarized", "inductor", "fuse", "crystal", "diode", "led", "zener", "schottky", "npn", "pnp", "nmos",
    "pmos", "button", "switch", "box", "conn",
];

pub fn template(name: &str) -> Option<Template> {
    let two = |graphics: Vec<Graphic>, slots: Vec<Slot>, names: &'static str| Template {
        graphics,
        slots,
        hide_pin_names: true,
        hide_pin_numbers: true,
        slot_names: names,
    };
    let tr = |graphics: Vec<Graphic>, slots: Vec<Slot>, names: &'static str| Template {
        graphics,
        slots,
        hide_pin_names: true,
        hide_pin_numbers: false,
        slot_names: names,
    };
    Some(match name {
        "resistor" => two(vec![rect(-40, -100, 40, 100, W10, None)], vslots(50), "1, 2"),
        "capacitor" => two(
            vec![poly(&[(-80, -30), (80, -30)], Some(20), None), poly(&[(-80, 30), (80, 30)], Some(20), None)],
            vslots(120),
            "1, 2",
        ),
        "cap_polarized" => two(
            vec![
                rect(-80, -40, 80, -20, W10, None),
                arc((-80, 50), (0, 20), (80, 50), Some(20)),
                poly(&[(-60, -90), (-20, -90)], None, None),
                poly(&[(-40, -110), (-40, -70)], None, None),
            ],
            vec![slot(0, -150, 270, 110), slot(0, 150, 90, 120)],
            "+, -",
        ),
        "inductor" => two(
            (0..4).map(|i| arc((0, -100 + i * 50), (25, -75 + i * 50), (0, -50 + i * 50), None)).collect(),
            vslots(50),
            "1, 2",
        ),
        "fuse" => two(vec![rect(-30, -80, 30, 80, W10, None), poly(&[(0, -100), (0, 100)], None, None)], vslots(70), "1, 2"),
        "crystal" => two(
            vec![
                rect(-30, -60, 30, 60, W10, None),
                poly(&[(-60, -50), (-60, 50)], Some(15), None),
                poly(&[(60, -50), (60, 50)], Some(15), None),
            ],
            vec![slot(-150, 0, 0, 90), slot(150, 0, 180, 90)],
            "1, 2",
        ),
        "diode" | "led" | "zener" | "schottky" => {
            let mut g = diode_body();
            match name {
                "zener" => g.push(poly(&[(-70, 60), (-50, 50), (-50, -50), (-30, -60)], W10, None)),
                "schottky" => g.push(poly(&[(-80, 30), (-80, 50), (-50, 50), (-50, -50), (-20, -50), (-20, -30)], W10, None)),
                _ => g.push(poly(&[(-50, -50), (-50, 50)], W10, None)),
            }
            if name == "led" {
                g.push(poly(&[(-30, -70), (10, -110)], None, None));
                g.push(poly(&[(10, -110), (-15, -105), (5, -85), (10, -110)], None, Some(Fill::Outline)));
                g.push(poly(&[(20, -70), (60, -110)], None, None));
                g.push(poly(&[(60, -110), (35, -105), (55, -85), (60, -110)], None, Some(Fill::Outline)));
            }
            two(g, vec![slot(-150, 0, 0, 100), slot(150, 0, 180, 100)], "K, A")
        }
        "npn" => {
            let mut g = bjt_body();
            g.push(poly(&[(0, -25), (100, -100)], None, None));
            g.push(poly(&[(0, 25), (100, 100)], None, None));
            g.push(poly(&[(85, 89), (44, 77), (62, 53), (85, 89)], None, Some(Fill::Outline)));
            tr(g, vec![slot(-200, 0, 0, 200), slot(100, -200, 270, 100), slot(100, 200, 90, 100)], "B, C, E")
        }
        "pnp" => {
            let mut g = bjt_body();
            g.push(poly(&[(0, -25), (100, -100)], None, None));
            g.push(poly(&[(0, 25), (100, 100)], None, None));
            g.push(poly(&[(20, -40), (61, -52), (43, -76), (20, -40)], None, Some(Fill::Outline)));
            tr(g, vec![slot(-200, 0, 0, 200), slot(100, 200, 90, 100), slot(100, -200, 270, 100)], "B, C, E")
        }
        "nmos" => tr(mos_body(true), vec![slot(-200, 0, 0, 160), slot(100, -200, 270, 100), slot(100, 200, 90, 100)], "G, D, S"),
        "pmos" => tr(mos_body(false), vec![slot(-200, 0, 0, 160), slot(100, 200, 90, 100), slot(100, -200, 270, 100)], "G, D, S"),
        "button" => two(
            vec![
                circle(-80, 0, 20, None),
                circle(80, 0, 20, None),
                poly(&[(-100, -50), (100, -50)], W10, None),
                poly(&[(0, -50), (0, -110)], None, None),
            ],
            vec![slot(-200, 0, 0, 100), slot(200, 0, 180, 100)],
            "1, 2",
        ),
        "switch" => two(
            vec![circle(-80, 0, 20, None), circle(80, 0, 20, None), poly(&[(-65, -15), (80, -80)], W10, None)],
            vec![slot(-200, 0, 0, 100), slot(200, 0, 180, 100)],
            "1, 2",
        ),
        _ => return None,
    })
}

/// Single-row connector body: pins on the left at a 100 mil pitch (eleceda's `Conn_01xNN`).
pub fn connector(n: usize) -> Template {
    let n = n.max(1) as i32;
    let y0 = -(((n - 1) * 100) / 2 / 50) * 50;
    let mut graphics = vec![rect(-50, y0 - 100, 50, y0 + (n - 1) * 100 + 100, W10, Some(Fill::Background))];
    let mut slots = Vec::new();
    for i in 0..n {
        let y = y0 + i * 100;
        graphics.push(rect(-50, y - 5, 0, y + 5, None, Some(Fill::Outline)));
        slots.push(slot(-200, y, 0, 150));
    }
    Template { graphics, slots, hide_pin_names: true, hide_pin_numbers: false, slot_names: "1…N" }
}

/// Dual-row connector, odd pins left and even pins right (eleceda's `Conn_02xNN_Odd_Even`).
pub fn connector_dual(rows: usize) -> Template {
    let rows = rows.max(1) as i32;
    let y0 = -(((rows - 1) * 100) / 2 / 50) * 50;
    let graphics = vec![rect(-100, y0 - 100, 100, y0 + (rows - 1) * 100 + 100, W10, Some(Fill::Background))];
    let mut slots = Vec::new();
    for i in 0..rows {
        slots.push(slot(-250, y0 + i * 100, 0, 150));
        slots.push(slot(250, y0 + i * 100, 180, 150));
    }
    Template { graphics, slots, hide_pin_names: true, hide_pin_numbers: false, slot_names: "1…2N" }
}

/// Symbol of a part drawn with a template; `pins` are (number, name, type) in slot order.
pub fn from_template(id: &str, name: &str, prefix: &str, t: Template, pins: &[(String, String, PinType, bool)]) -> SymbolDef {
    let pins: Vec<Pin> = pins
        .iter()
        .zip(&t.slots)
        .map(|((number, pname, ptype, hidden), s)| Pin {
            at: s.at,
            dir: s.dir,
            length: s.length,
            number: number.clone(),
            name: pname.clone(),
            ptype: *ptype,
            shape: "line".into(),
            unit: 0,
            hidden: hidden.then_some(true),
        })
        .collect();
    SymbolDef {
        id: id.into(),
        name: name.into(),
        description: None,
        keywords: None,
        ref_prefix: prefix.into(),
        unit_count: 1,
        power: None,
        hide_pin_names: t.hide_pin_names.then_some(true),
        hide_pin_numbers: t.hide_pin_numbers.then_some(true),
        pin_name_offset: None,
        open_pins_ok: None,
        graphics: t.graphics,
        pins,
        fields: std_fields(prefix, name),
    }
}

/// Reference, Value, Footprint and Datasheet fields (positions are set per instance by layout).
pub fn std_fields(prefix: &str, value: &str) -> Vec<Field> {
    vec![
        Field::new("Reference", prefix, pt(0, -150), true, Justify::Center),
        Field::new("Value", value, pt(0, 150), true, Justify::Center),
        Field::new("Footprint", "", pt(0, 0), false, Justify::Center),
        Field::new("Datasheet", "", pt(0, 0), false, Justify::Center),
    ]
}

fn power_def(id: String, name: &str, graphics: Vec<Graphic>, ptype: PinType, ref_prefix: &str, value_at: Pt) -> SymbolDef {
    SymbolDef {
        id,
        name: name.into(),
        description: None,
        keywords: Some("power".into()),
        ref_prefix: ref_prefix.into(),
        unit_count: 1,
        power: Some(true),
        hide_pin_names: Some(true),
        hide_pin_numbers: Some(true),
        pin_name_offset: None,
        open_pins_ok: None,
        graphics,
        pins: vec![Pin {
            at: pt(0, 0),
            dir: 90,
            length: 0,
            number: "1".into(),
            name: if ptype == PinType::PowerOut { "pwr".into() } else { String::new() },
            ptype,
            shape: "line".into(),
            unit: 0,
            hidden: (ptype == PinType::PowerIn).then_some(true),
        }],
        fields: vec![
            Field::new("Reference", ref_prefix, pt(0, 250), false, Justify::Center),
            Field::new("Value", name, value_at, true, Justify::Center),
            Field::new("Footprint", "", pt(0, 0), false, Justify::Center),
            Field::new("Datasheet", "", pt(0, 0), false, Justify::Center),
        ],
    }
}

/// Power symbol for a rail, drawn in its standard direction at rotation 0.
pub fn rail_symbol(name: &str, kind: RailKind) -> SymbolDef {
    let id = format!("power:{name}");
    match kind {
        RailKind::Ground => power_def(
            id,
            name,
            vec![poly(&[(0, 0), (0, 50), (50, 50), (0, 100), (-50, 50), (0, 50)], W10, None)],
            PinType::PowerIn,
            "#PWR",
            pt(0, 160),
        ),
        RailKind::Supply => power_def(
            id,
            name,
            vec![
                poly(&[(-30, -50), (0, -100)], W10, None),
                poly(&[(0, 0), (0, -100)], W10, None),
                poly(&[(0, -100), (30, -50)], W10, None),
            ],
            PinType::PowerIn,
            "#PWR",
            pt(0, -150),
        ),
        RailKind::Negative => power_def(
            id,
            name,
            vec![poly(&[(0, 0), (0, 100)], W10, None), poly(&[(-30, 50), (0, 100), (30, 50)], W10, None)],
            PinType::PowerIn,
            "#PWR",
            pt(0, 160),
        ),
    }
}

pub fn pwr_flag_symbol() -> SymbolDef {
    power_def(
        "power:PWR_FLAG".into(),
        "PWR_FLAG",
        vec![poly(&[(0, 0), (0, -50), (-50, -75), (0, -100), (50, -75), (0, -50)], W10, None)],
        PinType::PowerOut,
        "#FLG",
        pt(0, -150),
    )
}

/// Extent of a power symbol's drawing and value text relative to its connection point.
pub fn rail_extent(name: &str, kind: RailKind) -> crate::geom::Rect {
    let w = crate::geom::text_width(name, 50) / 2 + 10;
    match kind {
        RailKind::Ground | RailKind::Negative => crate::geom::Rect::new(-w.max(60), 0, w.max(60), 200),
        RailKind::Supply => crate::geom::Rect::new(-w.max(40), -190, w.max(40), 0),
    }
}
