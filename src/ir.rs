//! The elaborated design: a flat list of part instances and nets, plus the grouping and sheet
//! structure the layout engine uses. Everything downstream (ERC, layout, backends) reads this.

use crate::diag::Span;
use crate::symbol::boxgen::Side;
use crate::symbol::{PinType, RailKind, SymbolDef};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct PartPin {
    pub number: String,
    pub name: String,
    pub ptype: PinType,
    pub side: Option<Side>,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub struct PartDef {
    pub name: String,
    pub prefix: String,
    pub footprint: String,
    pub value: String,
    pub description: String,
    pub datasheet: String,
    pub mpn: String,
    pub fields: Vec<(String, String)>,
    /// Template name (`resistor`, `npn`, …), `box` or `conn`/`conn2`.
    pub symbol: String,
    pub pins: Vec<PartPin>,
    pub open_pins_ok: bool,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    /// Hierarchical path, e.g. `Power/reg/cin`.
    pub path: String,
    pub local: String,
    pub part: usize,
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub fields: Vec<(String, String)>,
    pub dnp: bool,
    pub group: usize,
    pub sheet: usize,
    pub span: Span,
    /// Net of each pin (index = part pin index); `None` when unconnected.
    pub pin_net: Vec<Option<usize>>,
    /// Pins marked `nc`.
    pub nc: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKind {
    Signal,
    Rail(RailKind),
}

#[derive(Debug, Clone)]
pub struct Net {
    pub name: String,
    pub kind: NetKind,
    /// (instance, pin index) in the order the source mentions them.
    pub pins: Vec<(usize, usize)>,
    /// `power_flag`: the net is driven from off-board (adds a PWR_FLAG).
    pub flagged: bool,
    /// The name comes from the source (not generated).
    pub named: bool,
    pub span: Option<Span>,
}

impl Net {
    pub fn is_rail(&self) -> bool {
        matches!(self.kind, NetKind::Rail(_))
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub sheet: usize,
    pub instances: Vec<usize>,
    /// `a near b.pin`: (a, b, pin index).
    pub near: Vec<(usize, usize, usize)>,
    /// Main direction of signal flow (0 = right, 270 = down).
    pub flow: u16,
    /// Draw a named frame around the group (module instances).
    pub framed: bool,
}

#[derive(Debug, Clone)]
pub struct SheetIr {
    pub name: String,
    pub groups: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Design {
    pub name: String,
    pub meta: BTreeMap<String, String>,
    pub parts: Vec<PartDef>,
    /// Symbol for each part (same index as `parts`).
    pub symbols: Vec<SymbolDef>,
    pub instances: Vec<Instance>,
    pub nets: Vec<Net>,
    pub groups: Vec<Group>,
    pub sheets: Vec<SheetIr>,
}

impl Design {
    pub fn pin(&self, inst: usize, pin: usize) -> &PartPin {
        &self.parts[self.instances[inst].part].pins[pin]
    }

    /// `R1.2` or `U1.VCC (3)` for messages.
    pub fn pin_label(&self, inst: usize, pin: usize) -> String {
        let p = self.pin(inst, pin);
        let r = &self.instances[inst].reference;
        if p.name.is_empty() || p.name == "~" || p.name == p.number {
            format!("{r}.{}", p.number)
        } else {
            format!("{r}.{} ({})", p.name, p.number)
        }
    }
}
