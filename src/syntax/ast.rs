//! Syntax tree of a `.wx` file.

use crate::diag::Span;

#[derive(Debug, Clone)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct File {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Use(Use),
    Part(PartDecl),
    Rail(RailDecl),
    Module(ModuleDecl),
    Sheet(SheetDecl),
    /// `title "…"`, `author "…"`, `revision "…"`, `company "…"`
    Meta { key: Name, value: Name },
    /// A statement outside any block: belongs to the implicit main sheet.
    Stmt(Stmt),
}

#[derive(Debug, Clone)]
pub struct Use {
    /// `use "parts/mcu.wx"`
    pub file: Option<Name>,
    /// `use std::{R, C}` → path `[std]`, names `[R, C]`; `use std::*` → names `None`.
    pub path: Vec<Name>,
    pub names: Option<Vec<Name>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PartDecl {
    pub name: Name,
    /// `key value` properties: prefix, footprint, value, description, datasheet, symbol, mpn, …
    pub props: Vec<(Name, Option<Name>)>,
    /// `field "LCSC" "C6186"`
    pub fields: Vec<(Name, Name)>,
    pub pins: Vec<PinDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PinDecl {
    pub numbers: Vec<Name>,
    pub name: Name,
    pub ptype: Option<Name>,
    pub side: Option<Name>,
    pub hidden: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RailDecl {
    pub name: Name,
    pub kind: Name,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ModuleDecl {
    pub name: Name,
    pub params: Vec<Name>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SheetDecl {
    pub name: Name,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `cin = C(10uF, 0805)` / `reg = Ldo(vin: VBUS)`
    Inst { name: Name, ty: Name, args: Vec<Arg>, span: Span },
    /// `a -- b.1, c.2 -- d`: everything joins one net; the order is a placement hint.
    Connect { groups: Vec<Vec<Endpoint>>, span: Span },
    /// `nc u.5, u.6`
    Nc { pins: Vec<Endpoint>, span: Span },
    /// `power_flag VBUS`
    PowerFlag { nets: Vec<Name>, span: Span },
    /// `layout { … }`
    Layout { hints: Vec<Hint>, span: Span },
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub key: Option<Name>,
    pub value: ArgValue,
}

#[derive(Debug, Clone)]
pub enum ArgValue {
    /// A bare value or name: `10k`, `0805`, `BAT54`, `VBUS`, `"text"`.
    Lit(Name),
    /// A pin or port: `u.VOUT`.
    Pin(Endpoint),
}

#[derive(Debug, Clone)]
pub enum Endpoint {
    /// A net, port or rail: `VBUS`, `vin`, `GND`.
    Net(Name),
    /// A pin of a part, or a port of a module instance: `u.VIN`, `c.1`, `reg.vout`.
    Pin { inst: Name, pin: Name },
    /// `nc`: the pins in this chain are intentionally unconnected.
    Nc(Span),
}

impl Endpoint {
    pub fn span(&self) -> Span {
        match self {
            Endpoint::Net(n) => n.span,
            Endpoint::Pin { inst, pin } => inst.span.to(pin.span),
            Endpoint::Nc(s) => *s,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Hint {
    /// `flow right|down|left|up`
    Flow(Name),
    /// `cin near u.VIN`
    Near { inst: Name, target: Endpoint },
}
