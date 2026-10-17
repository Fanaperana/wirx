//! Schematic symbols, mirroring eleceda's `SymbolDef` (`packages/core/src/model/symbol.ts`) so they
//! serialize straight into `.esch` files.

pub mod boxgen;
pub mod library;
pub mod templates;

use crate::geom::{bounds, pt, Orient, Pt, Rect, Rot};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

impl PinType {
    pub const ALL: [PinType; 12] = [
        PinType::Input,
        PinType::Output,
        PinType::Bidirectional,
        PinType::TriState,
        PinType::Passive,
        PinType::Free,
        PinType::Unspecified,
        PinType::PowerIn,
        PinType::PowerOut,
        PinType::OpenCollector,
        PinType::OpenEmitter,
        PinType::NoConnect,
    ];

    pub fn parse(s: &str) -> Option<PinType> {
        Some(match s {
            "input" | "in" => PinType::Input,
            "output" | "out" => PinType::Output,
            "bidirectional" | "bidi" | "io" | "inout" => PinType::Bidirectional,
            "tri_state" | "tristate" => PinType::TriState,
            "passive" | "pas" => PinType::Passive,
            "free" => PinType::Free,
            "unspecified" => PinType::Unspecified,
            "power_in" | "pwr_in" | "power" => PinType::PowerIn,
            "power_out" | "pwr_out" => PinType::PowerOut,
            "open_collector" | "open_drain" | "oc" | "od" => PinType::OpenCollector,
            "open_emitter" | "open_source" => PinType::OpenEmitter,
            "no_connect" | "nc" => PinType::NoConnect,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            PinType::Input => "input",
            PinType::Output => "output",
            PinType::Bidirectional => "bidirectional",
            PinType::TriState => "tri_state",
            PinType::Passive => "passive",
            PinType::Free => "free",
            PinType::Unspecified => "unspecified",
            PinType::PowerIn => "power_in",
            PinType::PowerOut => "power_out",
            PinType::OpenCollector => "open_collector",
            PinType::OpenEmitter => "open_emitter",
            PinType::NoConnect => "no_connect",
        }
    }

    pub fn index(self) -> usize {
        PinType::ALL.iter().position(|t| *t == self).unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    None,
    Outline,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Graphic {
    Polyline {
        points: Vec<Pt>,
        unit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fill: Option<Fill>,
    },
    Rect {
        start: Pt,
        end: Pt,
        unit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fill: Option<Fill>,
    },
    Circle {
        center: Pt,
        radius: i32,
        unit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fill: Option<Fill>,
    },
    Arc {
        start: Pt,
        mid: Pt,
        end: Pt,
        unit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fill: Option<Fill>,
    },
    Text {
        at: Pt,
        text: String,
        unit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<i32>,
    },
}

impl Graphic {
    pub fn points(&self) -> Vec<Pt> {
        match self {
            Graphic::Polyline { points, .. } => points.clone(),
            Graphic::Rect { start, end, .. } => vec![*start, *end],
            Graphic::Circle { center, radius, .. } => {
                vec![pt(center.x - radius, center.y - radius), pt(center.x + radius, center.y + radius)]
            }
            Graphic::Arc { start, mid, end, .. } => vec![*start, *mid, *end],
            Graphic::Text { at, .. } => vec![*at],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    /// Connection point: the outer tip of the pin.
    pub at: Pt,
    /// Direction from the tip toward the body.
    pub dir: Rot,
    pub length: i32,
    pub number: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ptype: PinType,
    pub shape: String,
    pub unit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Justify {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: String,
    /// Offset from the symbol origin, in sheet orientation.
    pub at: Pt,
    pub visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub justify: Option<Justify>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rot: Option<u16>,
}

impl Field {
    pub fn new(name: &str, value: &str, at: Pt, visible: bool, justify: Justify) -> Field {
        Field { name: name.into(), value: value.into(), at, visible, justify: Some(justify), rot: Some(0) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolDef {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    pub ref_prefix: String,
    pub unit_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_pin_names: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_pin_numbers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_name_offset: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_pins_ok: Option<bool>,
    pub graphics: Vec<Graphic>,
    pub pins: Vec<Pin>,
    pub fields: Vec<Field>,
}

impl SymbolDef {
    /// Bounds of the drawing (graphics only), in symbol space.
    pub fn body(&self) -> Rect {
        bounds(self.graphics.iter().flat_map(|g| g.points())).unwrap_or(Rect::new(-50, -50, 50, 50))
    }

    /// Bounds of drawing and pins, in symbol space.
    pub fn extent(&self) -> Rect {
        let mut r = self.body();
        for p in &self.pins {
            r = r.add_point(p.at);
        }
        r
    }

    pub fn is_power(&self) -> bool {
        self.power == Some(true)
    }
}

/// Where a pin ends up once its symbol is placed.
#[derive(Debug, Clone, Copy)]
pub struct PlacedPin {
    pub at: Pt,
    /// Direction pointing away from the body (where a wire should leave).
    pub out: Rot,
}

pub fn place_pin(pin: &Pin, o: Orient, origin: Pt) -> PlacedPin {
    PlacedPin { at: o.apply(pin.at) + origin, out: crate::geom::opposite(o.angle(pin.dir)) }
}

/// Power-symbol styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RailKind {
    Ground,
    Supply,
    Negative,
}

impl RailKind {
    pub fn parse(s: &str) -> Option<RailKind> {
        match s {
            "ground" | "gnd" => Some(RailKind::Ground),
            "supply" | "positive" | "pos" => Some(RailKind::Supply),
            "negative" | "neg" => Some(RailKind::Negative),
            _ => None,
        }
    }
    /// The direction the symbol extends from its connection point.
    pub fn natural_dir(self) -> Rot {
        match self {
            RailKind::Ground | RailKind::Negative => 270,
            RailKind::Supply => 90,
        }
    }
}
