//! Turns the elaborated design into drawn sheets.
//!
//! 1. Each group (a module instance, or the parts placed directly on a sheet) is laid out on
//!    its own: the part with the most pins anchors it, connected parts grow outward from the pins
//!    they attach to, and rail-only parts (decoupling caps) settle next to the pins they serve.
//! 2. Rails become power symbols at every pin, drawn upright (ground down, supplies up).
//! 3. Nets inside a group are routed with orthogonal wires on the 50 mil grid; nets leaving the
//!    group get one net label each.
//! 4. Groups are packed onto sheets, framed and named.

mod group;
pub use group::{GroupStats, Variant};
mod pack;
mod route;
mod text;

use crate::diag::Diagnostic;
use crate::geom::{Orient, Pt, Rect, Rot};
use crate::ir::Design;
use crate::symbol::{Field, SymbolDef};

#[derive(Debug, Clone)]
pub enum Item {
    Symbol {
        /// Stable identity, used to derive the item id.
        key: String,
        lib_id: String,
        at: Pt,
        orient: Orient,
        fields: Vec<Field>,
        inst: Option<usize>,
    },
    Wire {
        a: Pt,
        b: Pt,
    },
    Junction(Pt),
    NoConnect(Pt),
    Label {
        net: usize,
        text: String,
        at: Pt,
        rot: Rot,
        global: bool,
        shape: &'static str,
    },
    Block {
        name: String,
        rect: Rect,
    },
}

impl Item {
    pub fn translate(&mut self, d: Pt) {
        match self {
            Item::Symbol { at, .. } | Item::Junction(at) | Item::NoConnect(at) | Item::Label { at, .. } => *at = *at + d,
            Item::Wire { a, b } => {
                *a = *a + d;
                *b = *b + d;
            }
            Item::Block { rect, .. } => *rect = rect.translate(d),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    A4,
    A3,
    A2,
}

impl Paper {
    pub fn size(self) -> (i32, i32) {
        match self {
            Paper::A4 => (11693, 8268),
            Paper::A3 => (16535, 11693),
            Paper::A2 => (23386, 16535),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A3 => "A3",
            Paper::A2 => "A2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SheetOut {
    pub name: String,
    pub paper: Paper,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Schematic {
    pub sheets: Vec<SheetOut>,
    /// Layout quality per group, in group order.
    pub stats: Vec<GroupStats>,
    /// Every symbol definition the sheets use.
    pub symbols: Vec<SymbolDef>,
}

/// Which layout optimisations run (all by default; switch them off to measure each one).
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Try several placements per group and keep the best routed one.
    pub candidates: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { candidates: true }
    }
}

impl Options {
    /// Every optimisation off: the plain default layout.
    pub fn none() -> Self {
        Options { candidates: false }
    }
}

pub fn layout(d: &Design) -> (Schematic, Vec<Diagnostic>) {
    layout_with(d, &Options::default())
}

pub fn layout_with(d: &Design, opts: &Options) -> (Schematic, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut extra_symbols: Vec<SymbolDef> = Vec::new();
    let mut groups = Vec::new();
    let mut flags_done = std::collections::HashSet::new();
    for g in 0..d.groups.len() {
        if d.groups[g].instances.is_empty() {
            continue;
        }
        let out = group::layout_group(d, g, opts, &mut extra_symbols, &mut flags_done, &mut diags);
        groups.push(out);
    }
    let stats: Vec<GroupStats> = groups.iter().map(|g| g.stats.clone()).collect();
    let sheets = pack::pack(d, groups, &mut diags);
    let mut symbols: Vec<SymbolDef> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for s in d.symbols.iter().chain(extra_symbols.iter()) {
        if seen.insert(s.id.clone()) {
            symbols.push(s.clone());
        }
    }
    (Schematic { sheets, stats, symbols }, diags)
}
