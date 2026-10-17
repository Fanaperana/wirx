//! Placement and routing of one group, in group-local coordinates.

use super::route::{Grid, FOREIGN};
use super::Item;
use crate::diag::Diagnostic;
use crate::geom::{bounds, dir_vec, is_horizontal, opposite, pt, text_width, Orient, Pt, Rect, Rot, GRID};
use crate::ir::{Design, NetKind};
use crate::symbol::templates::{pwr_flag_symbol, rail_extent, rail_symbol};
use crate::symbol::{place_pin, Field, Justify, PlacedPin, RailKind, SymbolDef};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

mod bank;

/// Free space kept between neighbouring parts (mils).
const CLEAR: i32 = 100;
/// Length of the wire between a pin and the part placed in front of it.
const STUB: i32 = 100;
const TEXT: i32 = 50;

/// One way to place a group: the knobs candidate placement turns. Candidate 0 is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// Length of the wire between a pin and the part placed in front of it.
    pub gap: i32,
    /// Grow branches from the top pin down instead of bottom-up.
    pub top_down: bool,
    /// When a part must move off its pin's line, try the other direction first.
    pub flip: bool,
}

impl Variant {
    pub const DEFAULT: Variant = Variant { gap: 300, top_down: false, flip: false };

    /// The bounded, deterministic candidate set; the default comes first.
    pub fn candidates() -> Vec<Variant> {
        let mut v = vec![Variant::DEFAULT];
        for gap in [300, 400] {
            for top_down in [false, true] {
                for flip in [false, true] {
                    let c = Variant { gap, top_down, flip };
                    if c != Variant::DEFAULT {
                        v.push(c);
                    }
                }
            }
        }
        v
    }
}

impl GroupStats {
    /// The one quality order every optimisation uses (lower is better): unreachable pins,
    /// crossings, four-way points, bends, hugging parallel wires, length, then how far parts moved.
    pub fn rank(&self) -> (usize, usize, usize, usize, usize, i64, i64) {
        (self.unrouted, self.crossings, self.four_way, self.bends, self.hugs, self.length, self.displacement)
    }
}

/// How one group came out, for comparing candidates and for `wirx stats`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GroupStats {
    pub group: String,
    /// Index into `Variant::candidates()` of the placement used.
    pub candidate: usize,
    pub candidates_tried: usize,
    /// Wires of different nets crossing (stubs to labels and power symbols included).
    pub crossings: usize,
    pub bends: usize,
    /// Routed wire length, mils.
    pub length: i64,
    /// Wire pieces drawn (merging collinear pieces lowers it).
    pub wires: usize,
    /// Pairs of parallel wires of different nets one grid step apart.
    pub hugs: usize,
    /// Points where four wires meet (eleceda asks for two T-junctions instead).
    pub four_way: usize,
    /// Visible text or symbols covering wires, labels, other parts or other text.
    pub collisions: usize,
    /// Pins that had to be connected by label because no wire could reach them.
    pub unrouted: usize,
    /// Total movement of parts from the default candidate's positions, mils.
    pub displacement: i64,
    pub micros: u128,
}

pub struct GroupOut {
    pub group: usize,
    pub name: String,
    pub items: Vec<Item>,
    pub bbox: Rect,
    pub stats: GroupStats,
    /// Where each part ended up (for displacement).
    pub positions: HashMap<usize, Pt>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    TwoTerminal,
    Transistor,
    Fixed,
}

fn class_of(symbol: &str, pins: usize) -> Class {
    match symbol {
        "npn" | "pnp" | "nmos" | "pmos" => Class::Transistor,
        "box" | "conn" | "conn2" => Class::Fixed,
        _ if pins == 2 => Class::TwoTerminal,
        _ => Class::Fixed,
    }
}

/// Things drawn around a pin: power symbols, flags, labels, no-connect markers.
#[derive(Clone, Default)]
struct Decor {
    net: i32,
    wires: Vec<(Pt, Pt)>,
    pois: Vec<Pt>,
    block: Vec<Rect>,
    items: Vec<Item>,
}

impl Decor {
    fn translate(&mut self, d: Pt) {
        for w in &mut self.wires {
            *w = (w.0 + d, w.1 + d);
        }
        for p in &mut self.pois {
            *p = *p + d;
        }
        for r in &mut self.block {
            *r = r.translate(d);
        }
        for it in &mut self.items {
            it.translate(d);
        }
    }
}

#[derive(Clone)]
struct Placed {
    inst: usize,
    origin: Pt,
    orient: Orient,
    pins: Vec<PlacedPin>,
    body: Rect,
    /// Space this part claims: body and pins, text, decorations.
    rects: Vec<Rect>,
    decor: Vec<Decor>,
    fields: Vec<Field>,
    field_rects: Vec<Rect>,
}

impl Placed {
    fn translate(&mut self, d: Pt) {
        self.origin = self.origin + d;
        for p in &mut self.pins {
            p.at = p.at + d;
        }
        self.body = self.body.translate(d);
        for r in self.rects.iter_mut().chain(self.field_rects.iter_mut()) {
            *r = r.translate(d);
        }
        for dc in &mut self.decor {
            dc.translate(d);
        }
    }
}

struct Ctx<'a> {
    v: Variant,
    opts: super::Options,
    d: &'a Design,
    g: usize,
    in_group: HashSet<usize>,
    /// Net → the one pin in this group that carries its label.
    label_pin: HashMap<usize, (usize, usize)>,
    /// Net → pin that gets the PWR_FLAG.
    flag_pin: HashMap<usize, (usize, usize)>,
    extra_symbols: &'a mut Vec<SymbolDef>,
    diags: &'a mut Vec<Diagnostic>,
    /// Pins whose rail is drawn by a decoupling bank instead of their own power symbol.
    bank_pins: HashSet<(usize, usize)>,
}

pub fn label_rect(at: Pt, rot: Rot, text: &str) -> Rect {
    let len = text_width(text, TEXT) + 120;
    match rot {
        0 => Rect::new(at.x, at.y - 65, at.x + len, at.y + 40),
        180 => Rect::new(at.x - len, at.y - 65, at.x, at.y + 40),
        90 => Rect::new(at.x - 65, at.y - len, at.x + 40, at.y),
        _ => Rect::new(at.x - 65, at.y, at.x + 40, at.y + len),
    }
}

fn wire_rect(a: Pt, b: Pt) -> Rect {
    Rect::from_points(a, b).inflate(10)
}

/// Wire from a pin tip to where a symbol drawn in direction `natural` can sit without covering it.
/// Every symbol is joined to its pin by a wire, never set straight onto the pin tip: two connection
/// points touching without a wire do connect, but read as a gap and are easy to break by moving a part.
fn stub_to(t: Pt, out: Rot, natural: Rot, len: i32) -> (Vec<(Pt, Pt)>, Pt) {
    if out == natural || is_horizontal(out) {
        let s = t + dir_vec(out) * len;
        (vec![(t, s)], s)
    } else {
        let s1 = t + dir_vec(out) * STUB;
        let s = s1 + pt(150, 0);
        (vec![(t, s1), (s1, s)], s)
    }
}

fn hidden_field(name: &str, value: &str) -> Field {
    Field { name: name.into(), value: value.into(), at: pt(0, 0), visible: false, justify: None, rot: None }
}

fn text_rect(origin: Pt, f: &Field) -> Rect {
    let w = text_width(&f.value, TEXT);
    let x = origin.x + f.at.x
        - match f.justify {
            Some(Justify::Left) => 0,
            Some(Justify::Right) => w,
            _ => w / 2,
        };
    let y = origin.y + f.at.y;
    Rect::new(x, y - 30, x + w, y + 30)
}

impl Ctx<'_> {
    fn rail_of(&self, net: Option<usize>) -> Option<RailKind> {
        match net.map(|n| self.d.nets[n].kind) {
            Some(NetKind::Rail(k)) => Some(k),
            _ => None,
        }
    }

    fn class(&self, inst: usize) -> Class {
        let part = &self.d.parts[self.d.instances[inst].part];
        class_of(&part.symbol, part.pins.len())
    }

    fn sym(&self, inst: usize) -> &SymbolDef {
        &self.d.symbols[self.d.instances[inst].part]
    }

    /// Orientation forced by rails (ground pins down, supply pins up), if any rail pin exists.
    fn rail_orient(&self, inst: usize) -> Option<Orient> {
        let candidates: Vec<Orient> = match self.class(inst) {
            Class::Fixed => return Some(Orient::IDENTITY),
            Class::Transistor => vec![Orient::IDENTITY, Orient { rot: 0, mirror: true }],
            Class::TwoTerminal => [0, 90, 180, 270].map(Orient::rot).to_vec(),
        };
        let i = &self.d.instances[inst];
        let sym = self.sym(inst);
        let mut any = false;
        let mut best: Option<(i32, Orient)> = None;
        for o in candidates {
            let mut score = 0;
            for (k, pin) in sym.pins.iter().enumerate() {
                if let Some(kind) = self.rail_of(i.pin_net[k]) {
                    any = true;
                    let out = place_pin(pin, o, pt(0, 0)).out;
                    score += if out == kind.natural_dir() {
                        10
                    } else if out == opposite(kind.natural_dir()) {
                        -10
                    } else {
                        -2
                    };
                }
            }
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, o));
            }
        }
        if any { best.map(|b| b.1) } else { None }
    }

    /// Lay out a part at the origin with its decorations and text.
    fn make_placed(&mut self, inst: usize, orient: Orient) -> Placed {
        let d = self.d;
        let i = &d.instances[inst];
        let sym = &d.symbols[i.part];
        let pins: Vec<PlacedPin> = sym.pins.iter().map(|p| place_pin(p, orient, pt(0, 0))).collect();
        let body = orient.rect(sym.body());
        let mut rects = vec![bounds(pins.iter().map(|p| p.at)).map(|r| r.union(body)).unwrap_or(body)];
        let mut decor = Vec::new();
        // Neighbouring pins on the same rail (several GND pins) share one power symbol.
        // Stacked pins (several numbers drawn as one pin) share the visible pin's decoration.
        let mut in_run: HashSet<usize> = stacked_pins(&pins);
        if self.class(inst) == Class::Fixed {
            for run in rail_runs(d, inst, &pins) {
                in_run.extend(run.iter().copied());
                let n = i.pin_net[run[0]].unwrap();
                let NetKind::Rail(kind) = d.nets[n].kind else { continue };
                let out = pins[run[0]].out;
                let mut wires = Vec::new();
                let mut pois = Vec::new();
                let ends: Vec<Pt> = run.iter().map(|&k| pins[k].at + dir_vec(out) * STUB).collect();
                for (&k, &e) in run.iter().zip(&ends) {
                    wires.push((pins[k].at, e));
                    pois.extend([pins[k].at, e]);
                }
                let (first, last) = (ends[0], *ends.last().unwrap());
                wires.push((first, last));
                // The bus end the symbol does not use: a PWR_FLAG continues the bus from there, so its
                // wire never runs along the bus.
                let direction = |from: Pt, to: Pt| -> Rot {
                    let v = to - from;
                    if v.x > 0 { 0 } else if v.x < 0 { 180 } else if v.y < 0 { 90 } else { 270 }
                };
                let (s, free_end) = if out == kind.natural_dir() {
                    (ends[ends.len() / 2], (last, direction(first, last)))
                } else {
                    // Along the bus to the end nearest the symbol's direction, then one step out.
                    let toward = dir_vec(kind.natural_dir());
                    let (tip, other) = if (last - first).x * toward.x + (last - first).y * toward.y >= 0 { (last, first) } else { (first, last) };
                    let s = tip + toward * STUB;
                    wires.push((tip, s));
                    (s, (other, direction(tip, other)))
                };
                pois.push(s);
                let flag = run.iter().any(|&k| self.flag_pin.get(&n) == Some(&(inst, k)));
                let key = format!("pwr:{}:{}", i.path, sym.pins[run[0]].number);
                // Side buses: the flag leads straight out, away from the part. Top/bottom buses: it
                // continues the bus from its free end, so it never runs along the bus.
                let flag_at = if is_horizontal(out) { (s, out) } else { free_end };
                decor.push(self.rail_decor(n, kind, key, wires, pois, s, flag.then_some(flag_at)));
            }
        }
        for (k, pp) in pins.iter().enumerate() {
            let (t, out) = (pp.at, pp.out);
            let net = i.pin_net[k];
            if i.nc[k] {
                decor.push(Decor { net: FOREIGN, pois: vec![t], block: vec![Rect::point(t).inflate(40)], items: vec![Item::NoConnect(t)], ..Default::default() });
                continue;
            }
            let Some(n) = net else { continue };
            let netd = &d.nets[n];
            if in_run.contains(&k) {
                continue;
            }
            if netd.is_rail() && self.bank_pins.contains(&(inst, k)) {
                continue;
            }
            if let NetKind::Rail(kind) = netd.kind {
                // A symbol drawn up/down from a side pin crosses the neighbouring pin's row: leave that
                // pin room to get out by moving the symbol further away.
                let toward = dir_vec(kind.natural_dir());
                let crowded = pins.iter().enumerate().any(|(j, q)| j != k && q.out == out && q.at == t + toward * 100 && i.pin_net[j] != Some(n));
                let len = self.side_rail_len(inst, &pins, k, if crowded { 250 } else { STUB });
                let (wires, s) = stub_to(t, out, kind.natural_dir(), len);
                let flag = self.flag_pin.get(&n) == Some(&(inst, k));
                let key = format!("pwr:{}:{}", i.path, sym.pins[k].number);
                // On a top or bottom pin the flag goes sideways: to the side with more room, so it
                // never lands on a neighbouring pin's symbol.
                let side = if is_horizontal(out) {
                    out
                } else {
                    let room = |dir: i32| {
                        pins.iter()
                            .filter(|q| q.out == out && q.at != t && (q.at.x - t.x).signum() == dir)
                            .map(|q| (q.at.x - t.x).abs())
                            .min()
                            .unwrap_or(i32::MAX)
                    };
                    if room(1) >= room(-1) { 0 } else { 180 }
                };
                decor.push(self.rail_decor(n, kind, key, wires, vec![t, s], s, flag.then_some((s, side))));
            } else if self.label_pin.get(&n) == Some(&(inst, k)) {
                let s = t + dir_vec(out) * STUB;
                let mut dc = Decor { net: n as i32, wires: vec![(t, s)], pois: vec![t, s], ..Default::default() };
                dc.block.push(label_rect(s, out, &netd.name));
                dc.items.push(Item::Wire { a: t, b: s });
                dc.items.push(Item::Label { net: n, text: netd.name.clone(), at: s, rot: out, global: false, shape: "bidirectional" });
                decor.push(dc);
            } else if self.flag_pin.get(&n) == Some(&(inst, k)) {
                // On a pin facing down or sideways the flag stands to the side of the stub, far
                // enough that its name clears the part; the net's wire continues from the corner.
                let (wires, s) = match stub_to(t, out, 90, STUB) {
                    (w, _) if w.len() == 2 => {
                        let s1 = w[0].1;
                        (vec![(t, s1), (s1, s1 + pt(250, 0))], s1 + pt(250, 0))
                    }
                    other => other,
                };
                let mut pois = vec![t];
                pois.extend(wires.iter().map(|w| w.1));
                let mut dc = Decor { net: n as i32, wires: wires.clone(), pois, ..Default::default() };
                dc.block.push(Rect::new(-130, -190, 130, 0).translate(s));
                for (a, b) in &wires {
                    dc.items.push(Item::Wire { a: *a, b: *b });
                }
                dc.items.push(self.flag_item(n, s));
                decor.push(dc);
            }
        }
        for dc in &decor {
            rects.extend(dc.block.iter().copied());
            rects.extend(dc.wires.iter().map(|w| wire_rect(w.0, w.1)));
        }
        let (fields, field_rects) = self.fields(inst, orient, body, &pins, &decor);
        rects.extend(field_rects.iter().copied());
        Placed { inst, origin: pt(0, 0), orient, pins, body, rects, decor, fields, field_rects }
    }

    /// Stub length for the rail symbol on side pin `k` (at least `min`): the symbol stands up (or
    /// down) across the neighbouring rows, so it goes beyond their labels and rail symbols.
    fn side_rail_len(&self, inst: usize, pins: &[PlacedPin], k: usize, min: i32) -> i32 {
        let d = self.d;
        let i = &d.instances[inst];
        let Some(NetKind::Rail(kind)) = i.pin_net[k].map(|n| d.nets[n].kind) else { return min };
        let (t, out) = (pins[k].at, pins[k].out);
        if !is_horizontal(out) {
            return min;
        }
        // What the other pins on this side put in front of them.
        let toward = dir_vec(kind.natural_dir());
        let mut blocks: Vec<Rect> = Vec::new();
        for (j, q) in pins.iter().enumerate() {
            let Some(m) = i.pin_net[j] else { continue };
            let ahead = (q.at - t).x * toward.x + (q.at - t).y * toward.y;
            if j == k || q.out != out || i.nc[j] || Some(m) == i.pin_net[k] || ahead <= 0 || ahead > 400 {
                continue;
            }
            match d.nets[m].kind {
                NetKind::Rail(other) if other.natural_dir() != opposite(out) => {
                    let s = q.at + dir_vec(out) * self.side_rail_len(inst, pins, j, STUB);
                    blocks.push(rail_extent(&d.nets[m].name, other).translate(s));
                    blocks.push(wire_rect(q.at, s));
                }
                NetKind::Rail(_) => {}
                _ if self.label_pin.get(&m) == Some(&(inst, j)) => blocks.push(label_rect(q.at + dir_vec(out) * STUB, out, &d.nets[m].name)),
                _ => {}
            }
        }
        let name = &d.nets[i.pin_net[k].unwrap()].name;
        let mut len = min;
        while len < 2000 && blocks.iter().any(|b| rail_extent(name, kind).translate(t + dir_vec(out) * len).inflate(30).overlaps(b)) {
            len += 50;
        }
        len
    }

    /// A power symbol for rail `n` at `s`, reached through `wires`; optionally a PWR_FLAG on a wire
    /// leaving `flag.0` in direction `flag.1`.
    #[allow(clippy::too_many_arguments)]
    fn rail_decor(&mut self, n: usize, kind: RailKind, key: String, wires: Vec<(Pt, Pt)>, pois: Vec<Pt>, s: Pt, flag: Option<(Pt, Rot)>) -> Decor {
        let netd = &self.d.nets[n];
        let mut dc = Decor { net: n as i32, wires, pois, ..Default::default() };
        dc.block.push(rail_extent(&netd.name, kind).translate(s));
        let def = rail_symbol(&netd.name, kind);
        let value_at = def.fields[1].at;
        if !self.extra_symbols.iter().any(|x| x.id == def.id) {
            self.extra_symbols.push(def.clone());
        }
        dc.items.push(Item::Symbol {
            key,
            lib_id: def.id.clone(),
            at: s,
            orient: Orient::IDENTITY,
            fields: vec![
                Field { name: "Reference".into(), value: "#PWR?".into(), at: pt(0, 250), visible: false, justify: Some(Justify::Center), rot: Some(0) },
                Field { name: "Value".into(), value: netd.name.clone(), at: value_at, visible: true, justify: Some(Justify::Center), rot: Some(0) },
                hidden_field("Footprint", ""),
                hidden_field("Datasheet", ""),
            ],
            inst: None,
        });
        if let Some((from, dir)) = flag {
            let f = from + dir_vec(dir) * 200;
            dc.wires.push((from, f));
            dc.pois.push(f);
            dc.block.push(Rect::new(-130, -190, 130, 0).translate(f));
            dc.items.push(self.flag_item(n, f));
        }
        for (a, b) in &dc.wires {
            dc.items.push(Item::Wire { a: *a, b: *b });
        }
        dc
    }

    fn flag_item(&mut self, n: usize, at: Pt) -> Item {
        let def = pwr_flag_symbol();
        let value_at = def.fields[1].at;
        if !self.extra_symbols.iter().any(|x| x.id == def.id) {
            self.extra_symbols.push(def.clone());
        }
        Item::Symbol {
            key: format!("flag:{}", self.d.nets[n].name),
            lib_id: def.id,
            at,
            orient: Orient::IDENTITY,
            fields: vec![
                Field { name: "Reference".into(), value: "#FLG?".into(), at: pt(0, 100), visible: false, justify: Some(Justify::Center), rot: Some(0) },
                Field { name: "Value".into(), value: "PWR_FLAG".into(), at: value_at, visible: true, justify: Some(Justify::Center), rot: Some(0) },
                hidden_field("Footprint", ""),
                hidden_field("Datasheet", ""),
            ],
            inst: None,
        }
    }

    /// Reference and value text next to the body, clear of pins and decorations.
    fn fields(&self, inst: usize, orient: Orient, body: Rect, pins: &[PlacedPin], decor: &[Decor]) -> (Vec<Field>, Vec<Rect>) {
        let i = &self.d.instances[inst];
        let mut taken: Vec<Rect> = pins.iter().map(|p| Rect::point(p.at)).collect();
        for dc in decor {
            taken.extend(dc.block.iter().copied());
            taken.extend(dc.wires.iter().map(|w| Rect::from_points(w.0, w.1)));
        }
        // Lowest/highest point of anything within the body's columns.
        let over = |r: &Rect| r.x1 >= body.x0 && r.x0 <= body.x1;
        let top = taken.iter().filter(|r| over(r)).map(|r| r.y0).chain([body.y0]).min().unwrap();
        let bottom = taken.iter().filter(|r| over(r)).map(|r| r.y1).chain([body.y1]).max().unwrap();
        let (r_at, r_j, v_at, v_j) = match self.class(inst) {
            Class::TwoTerminal if body.w() <= body.h() => (pt(body.x1 + 40, -30), Justify::Left, pt(body.x1 + 40, 30), Justify::Left),
            Class::TwoTerminal => (pt(body.center().x, top - 50), Justify::Center, pt(body.center().x, bottom + 50), Justify::Center),
            Class::Transistor if orient.mirror => (pt(body.x0 - 30, -50), Justify::Right, pt(body.x0 - 30, 50), Justify::Right),
            Class::Transistor => (pt(body.x1 + 30, -50), Justify::Left, pt(body.x1 + 30, 50), Justify::Left),
            Class::Fixed => (pt(body.x0, top - 50), Justify::Left, pt(body.x0, bottom + 50), Justify::Left),
        };
        let mut fields = vec![
            Field { name: "Reference".into(), value: i.reference.clone(), at: r_at, visible: true, justify: Some(r_j), rot: Some(0) },
            Field { name: "Value".into(), value: i.value.clone(), at: v_at, visible: true, justify: Some(v_j), rot: Some(0) },
            hidden_field("Footprint", &i.footprint),
        ];
        let datasheet = i.fields.iter().find(|f| f.0 == "Datasheet").map(|f| f.1.clone()).unwrap_or_default();
        fields.push(hidden_field("Datasheet", &datasheet));
        for (k, v) in &i.fields {
            if k != "Datasheet" {
                fields.push(hidden_field(k, v));
            }
        }
        let rects = fields[..2].iter().map(|f| text_rect(pt(0, 0), f)).collect();
        (fields, rects)
    }
}

/// Runs of adjacent pins (100 mil apart, same side) on the same rail.
/// Pins drawn at the same point as an earlier pin of the same part (stacked pins).
fn stacked_pins(pins: &[PlacedPin]) -> HashSet<usize> {
    (0..pins.len()).filter(|&k| (0..k).any(|j| pins[j].at == pins[k].at)).collect()
}

fn rail_runs(d: &Design, inst: usize, pins: &[PlacedPin]) -> Vec<Vec<usize>> {
    let stacked = stacked_pins(pins);
    let i = &d.instances[inst];
    let rail = |k: usize| i.pin_net[k].filter(|&n| d.nets[n].is_rail() && !i.nc[k]);
    let mut by_side: HashMap<(usize, Rot), Vec<usize>> = HashMap::new();
    for k in 0..pins.len() {
        if stacked.contains(&k) {
            continue;
        }
        if let Some(n) = rail(k) {
            if let NetKind::Rail(kind) = d.nets[n].kind
                && pins[k].out == opposite(kind.natural_dir())
            {
                continue;
            }
            by_side.entry((n, pins[k].out)).or_default().push(k);
        }
    }
    let along = |p: &PlacedPin| if is_horizontal(p.out) { p.at.y } else { p.at.x };
    let mut runs = Vec::new();
    let mut keys: Vec<_> = by_side.keys().copied().collect();
    keys.sort();
    for key in keys {
        let mut ks = by_side[&key].clone();
        ks.sort_by_key(|&k| along(&pins[k]));
        let mut cur: Vec<usize> = Vec::new();
        for k in ks {
            let adjacent = cur.last().is_some_and(|&l| along(&pins[k]) - along(&pins[l]) == 100 && {
                let (a, b) = (pins[l].at, pins[k].at);
                if is_horizontal(key.1) { a.x == b.x } else { a.y == b.y }
            });
            if !adjacent && !cur.is_empty() {
                if cur.len() > 1 {
                    runs.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            cur.push(k);
        }
        if cur.len() > 1 {
            runs.push(cur);
        }
    }
    runs
}

fn collides(p: &Placed, d: Pt, occupied: &[Rect]) -> bool {
    p.rects.iter().any(|r| {
        let r = r.translate(d).inflate(CLEAR);
        occupied.iter().any(|o| r.overlaps(o))
    })
}

/// Offsets for a part placed in front of a pin: (along, across), nearest first.
fn front_offsets() -> &'static [(i32, i32)] {
    static CELL: OnceLock<Vec<(i32, i32)>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v = Vec::new();
        for a in (0..=3000).step_by(GRID as usize) {
            for p in (-4000..=4000).step_by(GRID as usize) {
                v.push((a, p));
            }
        }
        v.sort_by_key(|&(a, p): &(i32, i32)| (2 * a + p.abs() + i32::from(p < 0), a, p));
        v
    })
}

/// Offsets for a part hanging off a line (a pull-down under a wire): neighbours line up along it.
fn along_offsets() -> &'static [(i32, i32)] {
    static CELL: OnceLock<Vec<(i32, i32)>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v: Vec<(i32, i32)> = front_offsets().to_vec();
        v.sort_by_key(|&(a, p): &(i32, i32)| (a + 3 * p.abs() + i32::from(p < 0), a, p));
        v
    })
}

/// Offsets around a point, nearest first.
fn ring_offsets() -> &'static [Pt] {
    static CELL: OnceLock<Vec<Pt>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v = Vec::new();
        for x in (-6000..=6000).step_by(GRID as usize) {
            for y in (-6000..=6000).step_by(GRID as usize) {
                v.push(pt(x, y));
            }
        }
        v.sort_by_key(|p| (p.x.abs() + p.y.abs(), p.y.abs(), p.x, p.y));
        v
    })
}

fn place_near(proto: &Placed, desired: Pt, occupied: &[Rect]) -> Placed {
    let base = desired - proto.origin;
    for &off in ring_offsets() {
        if !collides(proto, base + off, occupied) {
            let mut p = proto.clone();
            p.translate(base + off);
            return p;
        }
    }
    let mut p = proto.clone();
    p.translate(base);
    p
}

/// Lay out a group: route every candidate placement independently and keep the best. A candidate
/// with more collisions, unreachable pins or four-way points than the default is rejected; the rest
/// are ranked by [`GroupStats::rank`] (crossings, bends, wire length, …, displacement from the
/// default), and only a strictly better one replaces the default.
pub fn layout_group(d: &Design, g: usize, opts: &super::Options, extra_symbols: &mut Vec<SymbolDef>, flags_done: &mut HashSet<usize>, diags: &mut Vec<Diagnostic>) -> GroupOut {
    let start = std::time::Instant::now();
    let candidates = if opts.candidates { Variant::candidates() } else { vec![Variant::DEFAULT] };
    let mut best: Option<(GroupOut, HashSet<usize>, Vec<Diagnostic>)> = None;
    let mut base_positions: HashMap<usize, Pt> = HashMap::new();
    let mut base: Option<GroupStats> = None;
    for (ci, &v) in candidates.iter().enumerate() {
        let mut flags = flags_done.clone();
        let mut local_diags = Vec::new();
        let mut out = layout_candidate(d, g, v, opts, extra_symbols, &mut flags, &mut local_diags);
        if ci == 0 {
            base_positions = out.positions.clone();
            base = Some(out.stats.clone());
        }
        out.stats.candidate = ci;
        out.stats.displacement = out.positions.iter().map(|(i, p)| base_positions.get(i).map(|b| b.manhattan(*p) as i64).unwrap_or(0)).sum();
        let rank = |s: &GroupStats| s.rank();
        let better = match &best {
            None => true,
            Some((b, _, _)) => {
                let base = base.as_ref().unwrap();
                let valid = out.stats.collisions <= base.collisions && out.stats.unrouted <= base.unrouted && out.stats.four_way <= base.four_way;
                valid && rank(&out.stats) < rank(&b.stats)
            }
        };
        if better {
            best = Some((out, flags, local_diags));
        }
    }
    let (mut out, flags, local_diags) = best.unwrap();
    *flags_done = flags;
    diags.extend(local_diags);
    out.stats.candidates_tried = candidates.len();
    out.stats.micros = start.elapsed().as_micros();
    out
}

fn layout_candidate(d: &Design, g: usize, v: Variant, opts: &super::Options, extra_symbols: &mut Vec<SymbolDef>, flags_done: &mut HashSet<usize>, diags: &mut Vec<Diagnostic>) -> GroupOut {
    let group = &d.groups[g];
    let in_group: HashSet<usize> = group.instances.iter().copied().collect();

    // Which pin of each outgoing net carries the label, and where flags go.
    let mut label_pin = HashMap::new();
    let mut flag_pin = HashMap::new();
    for (n, net) in d.nets.iter().enumerate() {
        let here: Vec<(usize, usize)> = net.pins.iter().copied().filter(|p| in_group.contains(&p.0)).collect();
        if here.is_empty() {
            continue;
        }
        let fixed = |p: &(usize, usize)| {
            let part = &d.parts[d.instances[p.0].part];
            class_of(&part.symbol, part.pins.len()) == Class::Fixed
        };
        // Labels and flags go on visible pins: a stacked (hidden) pin is drawn by its visible twin.
        let visible: Vec<(usize, usize)> = here.iter().copied().filter(|p| !d.pin(p.0, p.1).hidden).collect();
        let choose = if visible.is_empty() { &here } else { &visible };
        if !net.is_rail() && here.len() < net.pins.len() {
            let pick = choose.iter().find(|p| fixed(p)).or(choose.first()).copied().unwrap();
            label_pin.insert(n, pick);
        }
        if net.flagged && !flags_done.contains(&n) {
            let mut candidates: Vec<&(usize, usize)> = choose.iter().filter(|p| label_pin.get(&n) != Some(p) && !d.instances[p.0].nc[p.1]).collect();
            // A signal net's flag goes on a small part's pin: on a chip's edge, between pins 100 mil
            // apart, its symbol would block the neighbouring pins.
            if !net.is_rail() {
                candidates.sort_by_key(|p| fixed(p));
            } else {
                // A rail's flag stands beside its power symbol: pick the pin with the most room
                // beside it along its edge (neighbouring pins' symbols are 100 mil away).
                let room = |p: &(usize, usize)| -> i32 {
                    let sym = &d.symbols[d.instances[p.0].part];
                    let me = &sym.pins[p.1];
                    let along = |q: &crate::symbol::Pin| if me.dir == 0 || me.dir == 180 { q.at.y } else { q.at.x };
                    let side = |dir: i32| {
                        sym.pins.iter().filter(|q| q.dir == me.dir && q.at != me.at && (along(q) - along(me)).signum() == dir).map(|q| (along(q) - along(me)).abs()).min().unwrap_or(10_000)
                    };
                    side(1).max(side(-1))
                };
                candidates.sort_by_key(|p| std::cmp::Reverse(room(p).min(1000)));
            }
            if let Some(p) = candidates.first() {
                flag_pin.insert(n, **p);
                flags_done.insert(n);
            }
        }
    }

    let mut ctx = Ctx { v, opts: *opts, d, g, in_group, label_pin, flag_pin, extra_symbols, diags, bank_pins: HashSet::new() };
    let mut placed: HashMap<usize, Placed> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    let mut occupied: Vec<Rect> = Vec::new();
    // Parts hanging off a parent pin: (part, its pin, the parent pin).
    let mut hangs: Vec<(usize, usize, PlacedPin)> = Vec::new();

    let rail_only = |i: usize| {
        let inst = &d.instances[i];
        let mut rail = false;
        for k in 0..inst.pin_net.len() {
            match inst.pin_net[k].map(|n| &d.nets[n]) {
                Some(n) if n.is_rail() => rail = true,
                Some(n) if n.pins.len() > 1 && !inst.nc[k] => return false,
                _ => {}
            }
        }
        rail
    };
    let near: HashMap<usize, (usize, usize)> = group.near.iter().map(|&(a, b, k)| (a, (b, k))).collect();

    let commit = |p: Placed, placed: &mut HashMap<usize, Placed>, order: &mut Vec<usize>, occupied: &mut Vec<Rect>| {
        occupied.extend(p.rects.iter().copied());
        order.push(p.inst);
        placed.insert(p.inst, p);
    };

    // Where each part's outgoing nets land on a chip outside the group: (side of the chip, height
    // on it). A group is drawn beside the chip side its nets reach, so clusters go left to right
    // in the order that keeps their wires to that side from crossing: on the chip's left, the
    // cluster for the lowest pin nearest the chip (rightmost); on its right, mirrored.
    let partner_pins = |i: usize| -> Vec<(Rot, i32)> {
        let inst = &d.instances[i];
        let mut out = Vec::new();
        for n in inst.pin_net.iter().flatten().copied() {
            if d.nets[n].is_rail() || !ctx.label_pin.contains_key(&n) {
                continue;
            }
            for &(j, k) in &d.nets[n].pins {
                if ctx.in_group.contains(&j) || ctx.class(j) != Class::Fixed {
                    continue;
                }
                let pin = place_pin(&ctx.sym(j).pins[k], Orient::IDENTITY, pt(0, 0));
                out.push((pin.out, pin.at.y));
            }
        }
        out
    };
    let facing: i32 = group.instances.iter().flat_map(|&i| partner_pins(i)).map(|(out, _)| match out {
        180 => 1,
        0 => -1,
        _ => 0,
    }).sum::<i32>().signum();
    let cluster_key: HashMap<usize, (bool, i32)> = group
        .instances
        .iter()
        .map(|&i| (i, partner_pins(i).iter().map(|p| p.1).min().map_or((false, 0), |y| (true, -facing * y))))
        .collect();

    // Clusters of signal-connected parts, each grown from its biggest part.
    loop {
        let anchor = group
            .instances
            .iter()
            .copied()
            .filter(|i| !placed.contains_key(i) && !rail_only(*i))
            .max_by_key(|&i| (d.parts[d.instances[i].part].pins.len(), cluster_key[&i], std::cmp::Reverse(i)));
        let Some(anchor) = anchor else { break };
        let orient = ctx.rail_orient(anchor).unwrap_or(Orient::IDENTITY);
        let proto = ctx.make_placed(anchor, orient);
        let p = if occupied.is_empty() {
            proto
        } else {
            let all = occupied.iter().copied().reduce(Rect::union).unwrap();
            let ext = proto.rects.iter().copied().reduce(Rect::union).unwrap();
            let desired = pt(snap50(all.x1 + 400 - ext.x0), snap50(all.y0 - ext.y0));
            place_near(&proto, desired, &occupied)
        };
        commit(p, &mut placed, &mut order, &mut occupied);

        // Grow outward pin by pin.
        let mut qi = order.len() - 1;
        while qi < order.len() {
            let parent = order[qi];
            qi += 1;
            let pp = placed[&parent].pins.clone();
            let mut pin_order: Vec<usize> = (0..pp.len()).collect();
            pin_order.sort_by_key(|&k| {
                let side = match pp[k].out {
                    180 => 0,
                    0 => 1,
                    90 => 2,
                    _ => 3,
                };
                // Bottom-up on the sides: parts hanging down off lower pins take the near spots, so
                // the wires of the pins above pass over them instead of crossing.
                (side, if is_horizontal(pp[k].out) { if ctx.v.top_down { pp[k].at.y } else { -pp[k].at.y } } else { pp[k].at.x })
            });
            for k in pin_order {
                let Some(n) = d.instances[parent].pin_net[k] else { continue };
                if d.nets[n].is_rail() || d.instances[parent].nc[k] {
                    continue;
                }
                for &(ci, ck) in &d.nets[n].pins {
                    if ci == parent || placed.contains_key(&ci) || !ctx.in_group.contains(&ci) || rail_only(ci) {
                        continue;
                    }
                    // A `near` hint picks the parent pin itself.
                    let (anchor_pin, anchor_out) = match near.get(&ci).and_then(|&(b, bk)| placed.get(&b).map(|p| p.pins[bk])) {
                        Some(hp) => (hp.at, hp.out),
                        None => (pp[k].at, pp[k].out),
                    };
                    let p = match bridge_child(&mut ctx, ci, ck, parent, &pp, k, &occupied) {
                        Some(p) => p,
                        None => place_child(&mut ctx, ci, ck, anchor_pin, anchor_out, &occupied),
                    };
                    let straight = p.pins[ck].at.y == anchor_pin.y || p.pins[ck].at.x == anchor_pin.x;
                    let tip = p.pins[ck].at;
                    if is_horizontal(anchor_out) && !is_horizontal(p.pins[ck].out) {
                        hangs.push((ci, ck, PlacedPin { at: anchor_pin, out: anchor_out }));
                    }
                    commit(p, &mut placed, &mut order, &mut occupied);
                    if straight {
                        occupied.push(wire_rect(anchor_pin, tip));
                    }
                }
            }
        }
    }

    uncross_hangs(&mut placed, &order, &hangs);

    // Decoupling capacitors go in banks (see `bank`): drawn at their pin once the chips are placed,
    // or, with no pin to serve, as one unit in the row below. A lone capacitor with no pin to draw
    // it at stays an ordinary rail-only part.
    let banks = ctx.plan_banks(&near);
    let banked: HashSet<usize> = banks.iter().filter(|b| b.pin.is_some() || b.caps.len() > 1).flat_map(|b| b.caps.iter().copied()).collect();

    // Rail-only parts (bulk caps, regulators between rails): a `near` hint puts one at its pin;
    // the rest stand in one tidy row under the circuit, supplies on top and grounds on one baseline.
    let mut row: Vec<Vec<Placed>> = Vec::new();
    let mut attached: HashSet<usize> = HashSet::new();
    for &i in &group.instances {
        if placed.contains_key(&i) || banked.contains(&i) {
            continue;
        }
        let orient = ctx.rail_orient(i).unwrap_or(Orient::IDENTITY);
        let proto = ctx.make_placed(i, orient);
        if let Some(t) = near.get(&i).and_then(|&(b, k)| placed.get(&b).map(|p| p.pins[k])) {
            let p = place_at_pin(&proto, t, &occupied);
            commit(p, &mut placed, &mut order, &mut occupied);
        } else if banks.iter().any(|b| b.pin.is_some_and(|p| p.0 == i)) {
            // A chip between rails (a regulator) brings the banks at its pins along into the row.
            let mut local = HashMap::from([(i, proto.clone())]);
            let mut local_order = vec![i];
            let mut local_occ = proto.rects.clone();
            for b in banks.iter().filter(|b| b.pin.is_some_and(|p| p.0 == i)) {
                if ctx.attach_bank(b, &mut local, &mut local_order, &mut local_occ) {
                    attached.insert(b.caps[0]);
                }
            }
            row.push(local_order.iter().map(|k| local.remove(k).unwrap()).collect());
        } else {
            row.push(vec![proto]);
        }
    }
    // A long bank with no pin is split into shorter ones no wider than the circuit, so it does not
    // widen the group (and push its neighbours away from its pins).
    let limit = occupied.iter().copied().reduce(Rect::union).map_or(i32::MAX, |r| r.w());
    for b in banks.iter().filter(|b| b.pin.is_none() && b.caps.len() > 1) {
        let mut parts = 1;
        loop {
            let size = b.caps.len().div_ceil(parts);
            let units: Vec<Vec<Placed>> =
                b.caps.chunks(size).map(|caps| ctx.bank_unit(&bank::Bank { caps: caps.to_vec(), ..*b }, true, false).0).collect();
            let widest = units.iter().map(|u| u.iter().flat_map(|p| p.rects.iter().copied()).reduce(Rect::union).unwrap().w()).max().unwrap();
            if widest <= limit || size <= 2 {
                row.extend(units);
                break;
            }
            parts += 1;
        }
    }
    tidy(&ctx, &mut placed, &order);
    let mut occupied: Vec<Rect> = order.iter().flat_map(|i| placed[i].rects.iter().copied()).collect();
    place_row(&ctx, row, &mut placed, &mut order, &mut occupied);

    // Banks at their pins; those with no clear spot fall back to the row.
    let mut row: Vec<Vec<Placed>> = Vec::new();
    for b in banks.iter().filter(|b| b.pin.is_some() && !attached.contains(&b.caps[0])) {
        if ctx.attach_bank(b, &mut placed, &mut order, &mut occupied) {
            continue;
        }
        if b.caps.len() > 1 {
            row.push(ctx.bank_unit(b, true, false).0);
            continue;
        }
        let proto = ctx.unbank(b.caps[0]);
        match b.pin.and_then(|(c, k)| placed.get(&c).map(|p| p.pins[k])).filter(|_| near.contains_key(&b.caps[0])) {
            Some(t) => {
                let p = place_at_pin(&proto, t, &occupied);
                commit(p, &mut placed, &mut order, &mut occupied);
            }
            None => row.push(vec![proto]),
        }
    }
    place_row(&ctx, row, &mut placed, &mut order, &mut occupied);

    let occupied: Vec<Rect> = order.iter().flat_map(|i| placed[i].rects.iter().copied()).collect();
    route_group(&mut ctx, &placed, &order, &occupied)
}

/// A rail-only part placed in front of the pin it serves (a `near` hint).
fn place_at_pin(proto: &Placed, t: PlacedPin, occupied: &[Rect]) -> Placed {
    let ext = proto.rects.iter().copied().reduce(Rect::union).unwrap();
    let desired = t.at + dir_vec(t.out) * 300 - pt(ext.center().x, 0);
    place_near(proto, pt(snap50(desired.x), snap50(desired.y)), occupied)
}

/// Rail-only parts and banks with no pin to stand at: one row per side of the anchor they serve
/// (beside it, or under the circuit), supplies on top and grounds on one baseline. Each unit (a
/// part, or a bank's parts) moves as one.
fn place_row(ctx: &Ctx, mut row: Vec<Vec<Placed>>, placed: &mut HashMap<usize, Placed>, order: &mut Vec<usize>, occupied: &mut Vec<Rect>) {
    let d = ctx.d;
    let commit = |p: Placed, placed: &mut HashMap<usize, Placed>, order: &mut Vec<usize>, occupied: &mut Vec<Rect>| {
        occupied.extend(p.rects.iter().copied());
        order.push(p.inst);
        placed.insert(p.inst, p);
    };
    let extent = |u: &[Placed]| u.iter().flat_map(|p| p.rects.iter().copied()).reduce(Rect::union).unwrap();
    // A group made only of rail-connected parts (an LDO between two rails): its biggest part
    // anchors it, the rest go beside the pins they serve.
    if order.is_empty() && !row.is_empty() {
        let big = (0..row.len()).max_by_key(|&k| (row[k][0].pins.len(), std::cmp::Reverse(row[k][0].inst))).unwrap_or(0);
        for p in row.remove(big) {
            commit(p, placed, order, occupied);
        }
    }
    if row.is_empty() {
        return;
    }
    // The pin each part serves: a pin on the same supply rail, preferring the biggest part.
    let served = |p: &Placed, placed: &HashMap<usize, Placed>| -> Option<PlacedPin> {
        let inst = &d.instances[p.inst];
        let supply: Vec<usize> = inst.pin_net.iter().flatten().copied().filter(|&n| matches!(d.nets[n].kind, NetKind::Rail(RailKind::Supply))).collect();
        order.iter().find_map(|o| {
            let q = &placed[o];
            (0..q.pins.len()).find(|&k| d.instances[*o].pin_net[k].is_some_and(|n| supply.contains(&n))).map(|k| q.pins[k])
        })
    };
    let anchor_rects: Rect = order.first().map(|i| placed[i].rects.iter().copied().reduce(Rect::union).unwrap()).unwrap();
    // A cap serving a top or bottom pin goes to a side of the anchor that has no pins, when
    // there is one; otherwise under it.
    let free_side: Option<Rot> = order.first().and_then(|a| {
        let pins = &placed[a].pins;
        [180, 0].into_iter().find(|&o| !pins.iter().any(|p| p.out == o))
    });
    let side_of = |pin: Option<PlacedPin>| -> Option<Rot> {
        match pin {
            Some(t) if is_horizontal(t.out) => Some(t.out),
            Some(_) => free_side,
            None => None,
        }
    };
    // (side of the anchor, the pin served, the units in that row)
    type Side = (Option<Rot>, Option<PlacedPin>, Vec<Vec<Placed>>);
    let mut sides: Vec<Side> = Vec::new();
    for u in row {
        let pin = served(&u[0], placed);
        let side = side_of(pin);
        match sides.iter_mut().find(|(s, _, _)| *s == side) {
            Some((_, _, list)) => list.push(u),
            None => sides.push((side, pin, vec![u])),
        }
    }
    for (side, pin, mut list) in sides {
        let rail_name = |p: &Placed| {
            let inst = &d.instances[p.inst];
            inst.pin_net.iter().flatten().map(|&n| &d.nets[n]).find(|n| matches!(n.kind, NetKind::Rail(RailKind::Supply))).map(|n| n.name.clone()).unwrap_or_default()
        };
        list.sort_by_key(|u| (rail_name(&u[0]), u[0].inst));
        // A row under the circuit wraps at the circuit's width, so a long bank of decoupling
        // caps does not widen the group (and push its neighbours away from its pins).
        let limit = match side {
            None => occupied.iter().copied().reduce(Rect::union).map_or(i32::MAX, |r| r.w()),
            Some(_) => i32::MAX,
        };
        let (mut x, mut y, mut row_h) = (0, 0, 0);
        for u in &mut list {
            let ext = extent(u);
            if x > 0 && x + ext.w() > limit {
                (x, y, row_h) = (0, y + row_h + 2 * CLEAR, 0);
            }
            // A single part keeps its own height (supplies on top, grounds on one baseline); a
            // bank's top goes to the row's top.
            let dv = pt(snap50(x - ext.x0), snap50(y - if u.len() > 1 { ext.y0 } else { 0 }));
            for p in u.iter_mut() {
                p.translate(dv);
            }
            x = extent(u).x1 + 2 * CLEAR + 50;
            row_h = row_h.max(ext.h());
        }
        let row_ext = list.iter().map(|u| extent(u)).reduce(Rect::union).unwrap();
        let gap = 2 * CLEAR + 100;
        // Level with the pin it serves (or the anchor's middle), beside the chosen side.
        let level = match pin {
            Some(t) if is_horizontal(t.out) => t.at.y,
            _ => anchor_rects.center().y,
        };
        let base = match side {
            Some(180) => pt(anchor_rects.x0 - gap - row_ext.x1, level - row_ext.center().y),
            Some(_) => pt(anchor_rects.x1 + gap - row_ext.x0, level - row_ext.center().y),
            None => pt(anchor_rects.center().x - row_ext.w() / 2 - row_ext.x0, anchor_rects.y1 + gap - row_ext.y0),
        };
        let base = pt(snap50(base.x), snap50(base.y));
        let fits = |d: Pt, occupied: &[Rect]| list.iter().flatten().all(|p| !collides(p, d, occupied));
        let shift = ring_offsets().iter().map(|&o| base + o).find(|&d| fits(d, occupied)).unwrap_or(base);
        for u in list {
            for mut p in u {
                p.translate(shift);
                commit(p, placed, order, occupied);
            }
        }
    }
}

/// Line things up: nudge parts so connected pins share a row or column (straight wires instead
/// of jogs), and so ground and supply symbols of neighbouring parts sit on one baseline.
fn tidy(ctx: &Ctx, placed: &mut HashMap<usize, Placed>, order: &[usize]) {
    let d = ctx.d;
    let nets: Vec<Vec<(usize, usize)>> = d
        .nets
        .iter()
        .filter(|n| !n.is_rail())
        .map(|n| n.pins.iter().copied().filter(|p| ctx.in_group.contains(&p.0) && !d.instances[p.0].nc[p.1]).collect::<Vec<_>>())
        .filter(|t| t.len() >= 2)
        .collect();
    let mut nets_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ni, t) in nets.iter().enumerate() {
        for &(i, _) in t {
            let e = nets_of.entry(i).or_default();
            if !e.contains(&ni) {
                e.push(ni);
            }
        }
    }
    // A bent connection costs much more than a long straight one.
    let net_cost = |terms: &[(usize, usize)], placed: &HashMap<usize, Placed>| -> i64 {
        let pts: Vec<Pt> = terms.iter().map(|t| placed[&t.0].pins[t.1].at).collect();
        let mut in_tree = vec![false; pts.len()];
        in_tree[0] = true;
        let mut cost = 0i64;
        for _ in 1..pts.len() {
            let mut best = (i64::MAX, 0, 0);
            for a in 0..pts.len() {
                if !in_tree[a] {
                    continue;
                }
                for b in 0..pts.len() {
                    if !in_tree[b] {
                        let dd = pts[a].manhattan(pts[b]) as i64;
                        if dd < best.0 {
                            best = (dd, a, b);
                        }
                    }
                }
            }
            in_tree[best.2] = true;
            let (pa, pb) = (pts[best.1], pts[best.2]);
            let (dx, dy) = ((pa.x - pb.x).abs() as i64, (pa.y - pb.y).abs() as i64);
            cost += if dx != 0 && dy != 0 { 2000 + dx.min(dy) } else { 0 } + best.0 / 2;
        }
        cost
    };
    // Points where a part's ground (down) or supply (up) symbol attaches.
    let rail_points = |p: &Placed| -> Vec<(Rot, i32, i32)> {
        let inst = &d.instances[p.inst];
        p.pins
            .iter()
            .enumerate()
            .filter_map(|(k, pp)| match inst.pin_net[k].map(|n| d.nets[n].kind) {
                Some(NetKind::Rail(kind)) if pp.out == kind.natural_dir() => Some((pp.out, pp.at.x, pp.at.y)),
                _ => None,
            })
            .collect()
    };
    // Baseline cost: neighbouring symbols of the same direction at slightly different heights.
    let rail_cost = |placed: &HashMap<usize, Placed>| -> i64 {
        let pts: Vec<(Rot, i32, i32)> = order.iter().flat_map(|i| rail_points(&placed[i])).collect();
        let mut cost = 0;
        for a in 0..pts.len() {
            for b in a + 1..pts.len() {
                let (pa, pb) = (pts[a], pts[b]);
                if pa.0 == pb.0 && (pa.1 - pb.1).abs() <= 1000 && pa.2 != pb.2 && (pa.2 - pb.2).abs() <= 300 {
                    cost += 500;
                }
            }
        }
        cost
    };
    let anchor = order.first().copied();
    for _pass in 0..6 {
        let mut improved = false;
        for &p in order {
            if Some(p) == anchor {
                continue;
            }
            let my_nets = nets_of.get(&p).cloned().unwrap_or_default();
            let cost_of = |placed: &HashMap<usize, Placed>| -> i64 { my_nets.iter().map(|&n| net_cost(&nets[n], placed)).sum::<i64>() + rail_cost(placed) };
            let mut shifts: Vec<Pt> = Vec::new();
            for &n in &my_nets {
                for &(_, k) in nets[n].iter().filter(|t| t.0 == p) {
                    let at = placed[&p].pins[k].at;
                    for &(j, m) in nets[n].iter().filter(|t| t.0 != p) {
                        let other = placed[&j].pins[m].at;
                        shifts.push(pt(0, other.y - at.y));
                        shifts.push(pt(other.x - at.x, 0));
                    }
                }
            }
            for (dir, x, y) in rail_points(&placed[&p]) {
                for &o in order.iter().filter(|&&o| o != p) {
                    for (dir2, x2, y2) in rail_points(&placed[&o]) {
                        if dir == dir2 && (x - x2).abs() <= 1000 && (y2 - y).abs() <= 300 {
                            shifts.push(pt(0, y2 - y));
                        }
                    }
                }
            }
            shifts.retain(|s| *s != pt(0, 0) && s.x.abs() <= 1200 && s.y.abs() <= 1200);
            shifts.sort();
            shifts.dedup();
            if shifts.is_empty() {
                continue;
            }
            let others: Vec<Rect> = order.iter().filter(|&&o| o != p).flat_map(|o| placed[o].rects.iter().copied()).collect();
            let before = cost_of(placed);
            let mut best: Option<(i64, Pt)> = None;
            for s in shifts {
                if collides(&placed[&p], s, &others) {
                    continue;
                }
                let original = placed[&p].clone();
                placed.get_mut(&p).unwrap().translate(s);
                let after = cost_of(placed);
                placed.insert(p, original);
                if after < before && best.is_none_or(|b| after < b.0) {
                    best = Some((after, s));
                }
            }
            if let Some((_, s)) = best {
                placed.get_mut(&p).unwrap().translate(s);
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
}

/// Two parts hanging down (or up) off pins on one side of a part: the part on the pin further
/// from the hanging direction must stand further out, or its wire crosses the other's. Swap the
/// two across when they are the wrong way round and the swap fits.
fn uncross_hangs(placed: &mut HashMap<usize, Placed>, order: &[usize], hangs: &[(usize, usize, PlacedPin)]) {
    for a in 0..hangs.len() {
        for b in a + 1..hangs.len() {
            let ((ia, ka, pa), (ib, kb, pb)) = (hangs[a], hangs[b]);
            let (qa, qb) = (placed[&ia].pins[ka], placed[&ib].pins[kb]);
            if pa.out != pb.out || pa.at.x != pb.at.x || qa.out != qb.out || pa.at.y == pb.at.y {
                continue;
            }
            let ahead = dir_vec(pa.out).x;
            let (along_a, along_b) = ((qa.at.x - pa.at.x) * ahead, (qb.at.x - pb.at.x) * ahead);
            // Hanging down (pin facing up): the upper parent pin needs the further part.
            let down = qa.out == 90;
            let a_first = if down { pa.at.y < pb.at.y } else { pa.at.y > pb.at.y };
            if a_first == (along_a > along_b) || along_a == along_b {
                continue;
            }
            let dx = qb.at.x - qa.at.x;
            let (mut na, mut nb) = (placed[&ia].clone(), placed[&ib].clone());
            na.translate(pt(dx, 0));
            nb.translate(pt(-dx, 0));
            let others: Vec<Rect> = order.iter().filter(|&&o| o != ia && o != ib).flat_map(|o| placed[o].rects.iter().copied()).collect();
            if collides(&na, pt(0, 0), &others) || collides(&nb, pt(0, 0), &others) || collides(&na, pt(0, 0), &nb.rects) {
                continue;
            }
            placed.insert(ia, na);
            placed.insert(ib, nb);
        }
    }
}

fn snap50(v: i32) -> i32 {
    crate::geom::snap(v)
}

/// A two-pin part whose pins both go to the same side of one part (a crystal between XTAL1 and
/// XTAL2, a resistor between two neighbouring pins) stands parallel to that side, between them.
#[allow(clippy::too_many_arguments)]
fn bridge_child(ctx: &mut Ctx, ci: usize, ck: usize, parent: usize, pp: &[PlacedPin], k: usize, occupied: &[Rect]) -> Option<Placed> {
    let d = ctx.d;
    if ctx.class(ci) != Class::TwoTerminal {
        return None;
    }
    let ok = 1 - ck;
    let other_net = d.instances[ci].pin_net[ok]?;
    if d.nets[other_net].is_rail() {
        return None;
    }
    let out = pp[k].out;
    let k2 = (0..pp.len()).find(|&j| j != k && pp[j].out == out && d.instances[parent].pin_net[j] == Some(other_net))?;
    let (p1, p2) = (pp[k].at, pp[k2].at);
    let along_side = if is_horizontal(out) { |p: Pt| p.y } else { |p: Pt| p.x };
    let sym = ctx.sym(ci);
    // Orientation: pins spread along the side, in the same order as the parent's pins.
    let orient = [0, 90, 180, 270].map(Orient::rot).into_iter().find(|&o| {
        let (a, b) = (o.apply(sym.pins[ck].at), o.apply(sym.pins[ok].at));
        let across = if is_horizontal(out) { a.x == b.x } else { a.y == b.y };
        across && (along_side(a) < along_side(b)) == (along_side(p1) < along_side(p2))
    })?;
    let proto = ctx.make_placed(ci, orient);
    let mid = pt((p1.x + p2.x) / 2, (p1.y + p2.y) / 2);
    let pins_mid = pt((proto.pins[ck].at.x + proto.pins[ok].at.x) / 2, (proto.pins[ck].at.y + proto.pins[ok].at.y) / 2);
    let ext = proto.rects[0];
    let depth = if is_horizontal(out) { ext.w() } else { ext.h() } / 2;
    let target = mid + dir_vec(out) * (ctx.v.gap + depth) - pins_mid;
    let base = pt(snap50(target.x), snap50(target.y));
    let along = dir_vec(out);
    let across = dir_vec(crate::geom::norm_rot(out as i32 - 90)) * if ctx.v.flip { -1 } else { 1 };
    for &(a, p) in front_offsets() {
        let off = base + along * a + across * p;
        if !collides(&proto, off, occupied) {
            let mut placed = proto.clone();
            placed.translate(off);
            return Some(placed);
        }
    }
    None
}

/// Place part `ci` so its pin `ck` faces the parent pin at `at` (leaving in direction `out`).
fn place_child(ctx: &mut Ctx, ci: usize, ck: usize, at: Pt, out: Rot, occupied: &[Rect]) -> Placed {
    let want = opposite(out);
    let orient = ctx.rail_orient(ci).unwrap_or_else(|| {
        let sym = ctx.sym(ci);
        let candidates: Vec<Orient> = match ctx.class(ci) {
            Class::TwoTerminal => [0, 90, 180, 270].map(Orient::rot).to_vec(),
            Class::Transistor => vec![Orient::IDENTITY, Orient { rot: 0, mirror: true }],
            Class::Fixed => vec![Orient::IDENTITY],
        };
        candidates.iter().copied().find(|&o| place_pin(&sym.pins[ck], o, pt(0, 0)).out == want).unwrap_or(candidates[0])
    });
    let proto = ctx.make_placed(ci, orient);
    let q = proto.pins[ck];
    let tip = if q.out == want {
        at + dir_vec(out) * ctx.v.gap
    } else if is_horizontal(q.out) != is_horizontal(out) {
        at + dir_vec(out) * ctx.v.gap + dir_vec(opposite(q.out)) * 100
    } else {
        at + dir_vec(out) * (ctx.v.gap * 2)
    };
    let base = tip - q.at;
    let along = dir_vec(out);
    let across = dir_vec(crate::geom::norm_rot(out as i32 - 90)) * if ctx.v.flip { -1 } else { 1 };
    let hanging = is_horizontal(q.out) != is_horizontal(out);
    for &(a, p) in if hanging { along_offsets() } else { front_offsets() } {
        let off = base + along * a + across * p;
        if !collides(&proto, off, occupied) {
            let mut placed = proto.clone();
            placed.translate(off);
            return placed;
        }
    }
    place_near(&proto, base, occupied)
}

/// One routing attempt: the paths drawn per net (index into the group's net list) and the pins
/// that could not be reached.
#[derive(Default, Clone)]
struct Routing {
    paths: Vec<(usize, Vec<Pt>)>,
    failed: Vec<(usize, (usize, usize))>,
}

/// Quality of a routing attempt.
#[derive(Debug, Clone, Default)]
struct Metrics {
    /// (point, net, net) for wires of different nets crossing.
    crossings: Vec<(Pt, usize, usize)>,
    bends: usize,
    length: i64,
    /// Pairs of parallel wires of different nets one grid step apart.
    hugs: usize,
    /// Points where four wire directions meet (a lost junction dot turns them into crossings).
    four_way: usize,
}

impl Routing {
    fn metrics(&self, decor: &[(Pt, Pt, i32)]) -> Metrics {
        // Decoration wires (stubs to labels and power symbols) carry negative ids so two of them
        // never count against each other.
        let mut segs: Vec<(Pt, Pt, i64)> = decor.iter().map(|&(a, b, n)| (a, b, n as i64)).collect();
        for (n, path) in &self.paths {
            for w in path.windows(2) {
                segs.push((w[0], w[1], 1_000_000 + *n as i64));
            }
        }
        let inside = |p: Pt, s: Pt, t: Pt| crate::geom::on_segment(p, s, t) && p != s && p != t;
        let mut crossings = Vec::new();
        for &(a, b, n) in &segs {
            if a.y != b.y || a == b {
                continue;
            }
            for &(c, e, m) in &segs {
                if c.x != e.x || c == e || m == n || (n < 1_000_000 && m < 1_000_000) {
                    continue;
                }
                let p = pt(c.x, a.y);
                if inside(p, a, b) && inside(p, c, e) {
                    crossings.push((p, n.max(0) as usize, m.max(0) as usize));
                }
            }
        }
        let bends = self.paths.iter().map(|(_, p)| p.len().saturating_sub(2)).sum();
        let length = self.paths.iter().flat_map(|(_, p)| p.windows(2).map(|w| w[0].manhattan(w[1]) as i64)).sum();
        let mut hugs = 0;
        for (i, &(a, b, n)) in segs.iter().enumerate() {
            for &(c, e, m) in &segs[i + 1..] {
                if m == n || (n < 1_000_000 && m < 1_000_000) {
                    continue;
                }
                let horizontal = a.y == b.y && c.y == e.y && (a.y - c.y).abs() == GRID;
                let vertical = a.x == b.x && c.x == e.x && (a.x - c.x).abs() == GRID;
                let overlap = if horizontal {
                    a.x.min(b.x).max(c.x.min(e.x)) < a.x.max(b.x).min(c.x.max(e.x))
                } else if vertical {
                    a.y.min(b.y).max(c.y.min(e.y)) < a.y.max(b.y).min(c.y.max(e.y))
                } else {
                    false
                };
                hugs += usize::from(overlap);
            }
        }
        // Four-way points: count the wire directions leaving each point of each net.
        let mut dirs: HashMap<(i64, Pt), u8> = HashMap::new();
        let net_key = |n: i64| if n >= 1_000_000 { n } else { n + 1_000_000_000 };
        let mut points: Vec<Pt> = segs.iter().flat_map(|s| [s.0, s.1]).collect();
        points.sort();
        points.dedup();
        for &(a, b, n) in &segs {
            if a == b {
                continue;
            }
            for &p in &points {
                if !crate::geom::on_segment(p, a, b) {
                    continue;
                }
                let bit = |q: Pt| -> u8 {
                    if q.x > p.x { 1 } else if q.x < p.x { 2 } else if q.y > p.y { 4 } else { 8 }
                };
                let mut m = 0;
                if p != a {
                    m |= bit(a);
                }
                if p != b {
                    m |= bit(b);
                }
                *dirs.entry((net_key(n), p)).or_default() |= m;
            }
        }
        let _ = &dirs;
        // Decoration wires carry design net ids and paths group ids; a point is four-way when all
        // four directions are taken, whatever the source of the pieces.
        let mut by_point: HashMap<Pt, u8> = HashMap::new();
        for ((_, p), m) in &dirs {
            *by_point.entry(*p).or_default() |= *m;
        }
        let four_way = by_point.values().filter(|m| **m == 15).count();
        Metrics { crossings, bends, length, hugs, four_way }
    }
}

/// Route one net into `g` as a tree: start at its label pin (or first pin), then connect the
/// nearest remaining pin to any point of the tree, until all are reached.
fn route_net(g: &mut Grid, ni: usize, net: &(usize, Vec<(usize, usize)>), placed: &HashMap<usize, Placed>, label_pin: &HashMap<usize, (usize, usize)>, out: &mut Routing) {
    let (n, terms) = (net.0, &net.1);
    let tip = |t: &(usize, usize)| placed[&t.0].pins[t.1];
    let first = label_pin.get(&n).copied().filter(|p| terms.contains(p)).unwrap_or(terms[0]);
    let mut tree: HashSet<usize> = HashSet::new();
    let mut tree_pts: Vec<Pt> = vec![tip(&first).at];
    tree.extend(g.idx(tip(&first).at));
    for dc in &placed[&first.0].decor {
        if dc.net == n as i32 {
            for &(a, b) in &dc.wires {
                tree.extend(g.segment_cells(a, b));
            }
        }
    }
    let mut rest: Vec<(usize, usize)> = terms.iter().copied().filter(|t| *t != first).collect();
    while !rest.is_empty() {
        // Nearest remaining pin to the tree so far.
        let (ri, _) = rest.iter().enumerate().min_by_key(|(_, t)| tree_pts.iter().map(|q| q.manhattan(tip(t).at)).min().unwrap_or(0)).unwrap();
        let t = rest.remove(ri);
        let tp = tip(&t);
        if g.idx(tp.at).is_some_and(|c| tree.contains(&c)) {
            continue;
        }
        // A pin whose own decoration (a PWR_FLAG stub) fills the space in front of it is reached
        // through the far end of that decoration instead.
        let (gr, tr) = (&*g, &tree);
        let from_decor = || {
            let cost = |path: &Vec<Pt>| path.windows(2).map(|w| w[0].manhattan(w[1])).sum::<i32>() + 200 * path.len() as i32;
            placed[&t.0]
                .decor
                .iter()
                .filter(|dc| dc.net == n as i32)
                .flat_map(|dc| dc.pois.iter().copied())
                .filter(|&p| p != tp.at)
                .flat_map(|p| [tp.out, 0, 90, 180, 270].into_iter().filter_map(move |dir| gr.route(n as i32, p, dir, tr)))
                .min_by_key(|path| (cost(path), path.clone()))
        };
        match g.route(n as i32, tp.at, tp.out, &tree).or_else(from_decor) {
            Some(path) => {
                for w in path.windows(2) {
                    g.mark_wire(w[0], w[1], n as i32);
                    tree.extend(g.segment_cells(w[0], w[1]));
                }
                for dc in placed[&t.0].decor.iter().filter(|dc| dc.net == n as i32) {
                    for &(a, b) in &dc.wires {
                        tree.extend(g.segment_cells(a, b));
                    }
                }
                tree_pts.extend(path.iter().copied());
                out.paths.push((ni, path));
            }
            None => out.failed.push((ni, t)),
        }
    }
}

/// Iteration and time budget for crossing-driven rerouting (per placement candidate).
const REROUTE_ITERATIONS: usize = 12;
/// Work budget: nets rerouted per placement candidate. It counts work, not time, so the result is
/// the same on every machine and every run.
const REROUTE_WORK: usize = 200;
/// Stop after this many attempts in a row without improvement.
const REROUTE_PATIENCE: usize = 4;
/// Extra cost added around a contested crossing each time it recurs.
const HISTORY_STEP: u16 = 60;

impl Metrics {
    /// Lexicographic quality: unreachable pins, crossings, bends, length (lower is better).
    /// Same order as [`GroupStats::rank`].
    fn rank(&self, failed: usize) -> (usize, usize, usize, usize, usize, i64) {
        (failed, self.crossings.len(), self.four_way, self.bends, self.hugs, self.length)
    }
}

/// Rip up the nets involved in crossings and reroute them, with every other net in place, in a
/// few deterministic orders. Cells around crossings that keep coming back get more expensive
/// (congestion history). Only a strictly better result replaces `best`, so this never makes the
/// drawing worse.
#[allow(clippy::too_many_arguments)]
fn reroute_crossings(
    grid: &Grid,
    nets: &[(usize, Vec<(usize, usize)>)],
    mut best: Routing,
    base_order: &[usize],
    placed: &HashMap<usize, Placed>,
    label_pin: &HashMap<usize, (usize, usize)>,
    decor: &[(Pt, Pt, i32)],
) -> Routing {
    let mut work = 0;
    let mut best_metrics = best.metrics(decor);
    let mut history: HashMap<usize, u16> = HashMap::new();
    let mut current = best.clone();
    let mut current_metrics = best_metrics.clone();
    let mut stale = 0;
    for iteration in 0..REROUTE_ITERATIONS {
        if current_metrics.crossings.is_empty() || work >= REROUTE_WORK || stale >= REROUTE_PATIENCE {
            break;
        }
        // Nets (group indices) touching a crossing. Decoration wires carry design net ids; map
        // them back to the group's nets where they belong to one.
        let to_group = |id: usize| -> Option<usize> {
            if id >= 1_000_000 { Some(id - 1_000_000) } else { nets.iter().position(|n| n.0 == id) }
        };
        let affected: HashSet<usize> = current_metrics.crossings.iter().flat_map(|c| [to_group(c.1), to_group(c.2)]).flatten().collect();
        if affected.is_empty() {
            break;
        }
        for c in &current_metrics.crossings {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(cell) = grid.idx(c.0 + pt(dx * GRID, dy * GRID)) {
                        *history.entry(cell).or_insert(0) += HISTORY_STEP;
                    }
                }
            }
        }
        // Keep the other nets; rip up the affected ones.
        let mut g = grid.clone();
        for (&cell, &v) in &history {
            g.add_soft(cell, v);
        }
        let mut trial = Routing {
            paths: current.paths.iter().filter(|(n, _)| !affected.contains(n)).cloned().collect(),
            failed: current.failed.iter().filter(|f| !affected.contains(&f.0)).cloned().collect(),
        };
        for (n, path) in &trial.paths {
            for w in path.windows(2) {
                g.mark_wire(w[0], w[1], nets[*n].0 as i32);
            }
        }
        let mut order: Vec<usize> = base_order.iter().copied().filter(|n| affected.contains(n)).collect();
        match iteration % 3 {
            0 => {}
            1 => order.reverse(),
            _ => {
                let k = 1.min(order.len());
                order.rotate_left(k);
            }
        }
        for ni in order {
            route_net(&mut g, ni, &nets[ni], placed, label_pin, &mut trial);
            work += 1;
        }
        let m = trial.metrics(decor);
        if m.rank(trial.failed.len()) < best_metrics.rank(best.failed.len()) {
            best = trial.clone();
            best_metrics = m.clone();
            stale = 0;
        } else {
            stale += 1;
        }
        // Continue from the new attempt (even when it is not better) so the history keeps
        // pushing wires away from the contested spots.
        current = trial;
        current_metrics = m;
    }
    best
}

/// Cleanup, step 2: reroute each net on its own with all others in place, charging extra for
/// bends (jog removal) and for running right next to another net's parallel wire (track
/// separation). A new route is kept only when the whole group gets strictly better on
/// (unrouted, crossings, bends, hugging pairs, length).
fn polish(
    grid: &Grid,
    nets: &[(usize, Vec<(usize, usize)>)],
    mut best: Routing,
    placed: &HashMap<usize, Placed>,
    label_pin: &HashMap<usize, (usize, usize)>,
    decor: &[(Pt, Pt, i32)],
) -> Routing {
    let rank = |r: &Routing| {
        let m = r.metrics(decor);
        m.rank(r.failed.len())
    };
    let mut best_rank = rank(&best);
    for _pass in 0..2 {
        let mut improved = false;
        for ni in 0..nets.len() {
            let mut g = grid.clone();
            g.extra_bend = 60;
            g.hug = true;
            for (m, path) in &best.paths {
                if *m != ni {
                    for w in path.windows(2) {
                        g.mark_wire(w[0], w[1], nets[*m].0 as i32);
                    }
                }
            }
            let mut trial = Routing {
                paths: best.paths.iter().filter(|(m, _)| *m != ni).cloned().collect(),
                failed: best.failed.iter().filter(|f| f.0 != ni).cloned().collect(),
            };
            route_net(&mut g, ni, &nets[ni], placed, label_pin, &mut trial);
            let r = rank(&trial);
            if r < best_rank {
                best_rank = r;
                best = trial;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    best
}

/// Points a merge must never remove: pin tips and the connection points of decorations (stub
/// ends, label anchors, power symbols, flags).
fn anchor_points(order: &[usize], placed: &HashMap<usize, Placed>) -> HashSet<Pt> {
    let mut out = HashSet::new();
    for i in order {
        for p in &placed[i].pins {
            out.insert(p.at);
        }
        for dc in &placed[i].decor {
            out.extend(dc.pois.iter().copied());
            for &(a, b) in &dc.wires {
                out.insert(a);
                out.insert(b);
            }
        }
    }
    out
}

/// Cleanup, step 1: join collinear wire pieces of the same net that meet at a plain degree-two
/// point (no pin, junction, branch or decoration there).
fn merge_collinear(r: &Routing, nets: &[(usize, Vec<(usize, usize)>)], anchors: &HashSet<Pt>) -> Vec<(Pt, Pt)> {
    let mut out = Vec::new();
    let mut by_net: HashMap<usize, Vec<(Pt, Pt)>> = HashMap::new();
    for (n, path) in &r.paths {
        for w in path.windows(2) {
            if w[0] != w[1] {
                by_net.entry(nets[*n].0).or_default().push((w[0], w[1]));
            }
        }
    }
    let mut keys: Vec<usize> = by_net.keys().copied().collect();
    keys.sort();
    for k in keys {
        let mut segs = by_net.remove(&k).unwrap();
        loop {
            let mut merged = false;
            'outer: for i in 0..segs.len() {
                for j in i + 1..segs.len() {
                    let (a, b) = segs[i];
                    let (c, e) = segs[j];
                    let shared = [a, b].into_iter().find(|p| *p == c || *p == e);
                    let Some(p) = shared else { continue };
                    let collinear = (a.x == b.x && c.x == e.x && a.x == c.x) || (a.y == b.y && c.y == e.y && a.y == c.y);
                    if !collinear || anchors.contains(&p) {
                        continue;
                    }
                    // Exactly these two pieces end at p, and nothing passes through it.
                    let ends = segs.iter().filter(|s| s.0 == p || s.1 == p).count();
                    let through = segs.iter().any(|s| s.0 != p && s.1 != p && crate::geom::on_segment(p, s.0, s.1));
                    if ends != 2 || through {
                        continue;
                    }
                    let far1 = if a == p { b } else { a };
                    let far2 = if c == p { e } else { c };
                    // Overlapping pieces (going back over themselves) are not merged.
                    if (far1.x - p.x).signum() * (far2.x - p.x).signum() > 0 || (far1.y - p.y).signum() * (far2.y - p.y).signum() > 0 {
                        continue;
                    }
                    segs[i] = (far1, far2);
                    segs.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
            if !merged {
                break;
            }
        }
        out.extend(segs);
    }
    out
}

/// Which pins end up connected to which, rebuilt from geometry (wires join at their ends and at
/// any point lying on them). Component ids are numbered in pin order, so equal vectors mean equal
/// connectivity.
fn pin_partition(wires: &[(Pt, Pt)], decor: &[(Pt, Pt, i32)], anchors: &HashSet<Pt>, pins: &[Pt]) -> Vec<usize> {
    let segs: Vec<(Pt, Pt)> = wires.iter().copied().chain(decor.iter().map(|d| (d.0, d.1))).collect();
    let mut points: Vec<Pt> = segs.iter().flat_map(|s| [s.0, s.1]).chain(anchors.iter().copied()).chain(pins.iter().copied()).collect();
    points.sort();
    points.dedup();
    let index: HashMap<Pt, usize> = points.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let mut parent: Vec<usize> = (0..points.len()).collect();
    fn find(parent: &mut Vec<usize>, mut a: usize) -> usize {
        while parent[a] != a {
            parent[a] = parent[parent[a]];
            a = parent[a];
        }
        a
    }
    for &(a, b) in &segs {
        let (ra, rb) = (find(&mut parent, index[&a]), find(&mut parent, index[&b]));
        parent[ra] = rb;
        for (i, p) in points.iter().enumerate() {
            if crate::geom::on_segment(*p, a, b) {
                let (ri, rb) = (find(&mut parent, i), find(&mut parent, index[&a]));
                parent[ri] = rb;
            }
        }
    }
    let mut ids: HashMap<usize, usize> = HashMap::new();
    pins.iter()
        .map(|p| {
            let r = find(&mut parent, index[p]);
            let n = ids.len();
            *ids.entry(r).or_insert(n)
        })
        .collect()
}

fn route_group(ctx: &mut Ctx, placed: &HashMap<usize, Placed>, order: &[usize], occupied: &[Rect]) -> GroupOut {
    let d = ctx.d;
    let bbox = occupied.iter().copied().reduce(Rect::union).unwrap_or(Rect::new(0, 0, 0, 0)).inflate(800);
    let mut grid = Grid::new(bbox);
    let mut items: Vec<Item> = Vec::new();

    for &i in order {
        let p = &placed[&i];
        let sym = &d.symbols[d.instances[i].part];
        grid.block_rect(p.body);
        for r in &p.field_rects {
            grid.soft_rect(*r, 30);
        }
        for (k, pp) in p.pins.iter().enumerate() {
            let inward = dir_vec(opposite(pp.out));
            let len = sym.pins[k].length.max(GRID);
            for s in (GRID..=len).step_by(GRID as usize) {
                grid.block_rect(Rect::point(pp.at + inward * s));
            }
        }
    }
    for &i in order {
        let p = &placed[&i];
        for dc in &p.decor {
            for r in &dc.block {
                grid.block_rect(*r);
            }
        }
    }
    for &i in order {
        let p = &placed[&i];
        for (k, pp) in p.pins.iter().enumerate() {
            let net = d.instances[i].pin_net[k].filter(|_| !d.instances[i].nc[k]).map(|n| n as i32).unwrap_or(FOREIGN);
            grid.own(pp.at, net);
            grid.set_access(pp.at, opposite(pp.out));
        }
        for dc in &p.decor {
            for &(a, b) in &dc.wires {
                grid.mark_wire(a, b, dc.net);
            }
            for &q in &dc.pois {
                grid.own(q, dc.net);
            }
        }
    }

    // Route every net with two or more pins in this group: short nets first. When a pin cannot be
    // reached, rip everything up and retry with the failing nets first; keep the best attempt.
    let tip = |t: &(usize, usize)| placed[&t.0].pins[t.1];
    let nets: Vec<(usize, Vec<(usize, usize)>)> = d
        .nets
        .iter()
        .enumerate()
        .filter(|(_, net)| !net.is_rail())
        .map(|(n, net)| (n, net.pins.iter().copied().filter(|p| ctx.in_group.contains(&p.0) && !d.instances[p.0].nc[p.1]).collect::<Vec<_>>()))
        .filter(|(_, terms)| terms.len() >= 2)
        .collect();
    let decor_wires: Vec<(Pt, Pt, i32)> = order.iter().flat_map(|i| placed[i].decor.iter().flat_map(|dc| dc.wires.iter().map(move |w| (w.0, w.1, dc.net)))).collect();
    let span = |terms: &[(usize, usize)]| bounds(terms.iter().map(|t| tip(t).at)).map(|r| r.w() + r.h()).unwrap_or(0);
    let mut net_order: Vec<usize> = (0..nets.len()).collect();
    net_order.sort_by_key(|&i| (span(&nets[i].1), nets[i].0));
    let mut best: Option<Routing> = None;
    for _attempt in 0..4 {
        let mut g = grid.clone();
        let mut r = Routing::default();
        for &ni in &net_order {
            route_net(&mut g, ni, &nets[ni], placed, &ctx.label_pin, &mut r);
        }
        let done = r.failed.is_empty();
        let failing: HashSet<usize> = r.failed.iter().map(|f| f.0).collect();
        if best.as_ref().is_none_or(|b| r.failed.len() < b.failed.len()) {
            best = Some(r);
        }
        if done {
            break;
        }
        net_order.sort_by_key(|&i| !failing.contains(&i));
    }
    let mut best = best.unwrap_or_default();
    if ctx.opts.reroute {
        best = reroute_crossings(&grid, &nets, best, &net_order, placed, &ctx.label_pin, &decor_wires);
    }
    if ctx.opts.cleanup {
        best = polish(&grid, &nets, best, placed, &ctx.label_pin, &decor_wires);
    }
    let metrics = best.metrics(&decor_wires);
    let mut wires: Vec<(Pt, Pt)> = best.paths.iter().flat_map(|(_, p)| p.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>()).collect();
    if ctx.opts.cleanup {
        // Keep the merged wires only if every pin is connected exactly as before.
        let anchors = anchor_points(order, placed);
        let merged = merge_collinear(&best, &nets, &anchors);
        let pins: Vec<Pt> = order.iter().flat_map(|i| placed[i].pins.iter().map(|p| p.at)).collect();
        let before = pin_partition(&wires, &decor_wires, &anchors, &pins);
        if pin_partition(&merged, &decor_wires, &anchors, &pins) == before {
            wires = merged;
        }
    }
    let failed: Vec<(usize, (usize, usize))> = best.failed.iter().map(|&(ni, t)| (nets[ni].0, t)).collect();
    let mut extra_labels: Vec<Item> = Vec::new();
    let mut labeled: HashSet<usize> = ctx.label_pin.keys().copied().collect();
    let unrouted = failed.len();
    for (n, t) in failed {
        let net = &d.nets[n];
        let tp = tip(&t);
        ctx.diags.push(Diagnostic::warning(
            "unrouted",
            format!("could not draw a wire to {} on net `{}`; connected it by label instead", d.pin_label(t.0, t.1), net.name),
        ));
        extra_labels.push(Item::Label { net: n, text: net.name.clone(), at: tp.at, rot: tp.out, global: false, shape: "bidirectional" });
        if labeled.insert(n) {
            let terms = &nets.iter().find(|x| x.0 == n).unwrap().1;
            let ft = tip(&terms.iter().copied().find(|x| *x != t).unwrap_or(terms[0]));
            extra_labels.push(Item::Label { net: n, text: net.name.clone(), at: ft.at, rot: ft.out, global: false, shape: "bidirectional" });
        }
    }

    // Collect items.
    for &i in order {
        let p = &placed[&i];
        let inst = &d.instances[i];
        items.push(Item::Symbol {
            key: format!("inst:{}", inst.path),
            lib_id: d.symbols[inst.part].id.clone(),
            at: p.origin,
            orient: p.orient,
            fields: p.fields.clone(),
            inst: Some(i),
        });
        for dc in &p.decor {
            items.extend(dc.items.iter().cloned());
        }
    }
    let wire_count = wires.len();
    for (a, b) in &wires {
        items.push(Item::Wire { a: *a, b: *b });
    }
    items.extend(extra_labels);
    add_junctions(d, &mut items);
    super::text::place(d, &mut items, ctx.extra_symbols);

    let mut bb: Option<Rect> = None;
    for r in occupied {
        bb = Some(bb.map_or(*r, |b| b.union(*r)));
    }
    for (a, b) in &wires {
        let r = Rect::from_points(*a, *b);
        bb = Some(bb.map_or(r, |x| x.union(r)));
    }
    for r in super::text::field_rects(&items) {
        bb = Some(bb.map_or(r, |x| x.union(r)));
    }
    let group = &d.groups[ctx.g];
    let positions: HashMap<usize, Pt> = order.iter().map(|&i| (i, placed[&i].origin)).collect();
    let stats = GroupStats {
        group: group.name.clone(),
        crossings: metrics.crossings.len(),
        bends: metrics.bends,
        length: metrics.length,
        wires: wire_count,
        hugs: metrics.hugs,
        four_way: metrics.four_way,
        collisions: super::text::collisions(d, &items, ctx.extra_symbols),
        unrouted,
        ..Default::default()
    };
    GroupOut { group: ctx.g, name: group.name.clone(), items, bbox: bb.unwrap_or(Rect::new(0, 0, 100, 100)), stats, positions }
}

/// A junction dot wherever three or more connections meet at a point.
pub fn add_junctions(d: &Design, items: &mut Vec<Item>) {
    let mut pins: HashMap<Pt, u32> = HashMap::new();
    let mut ends: HashMap<Pt, u32> = HashMap::new();
    let mut segs = Vec::new();
    for it in items.iter() {
        match it {
            Item::Wire { a, b } => {
                *ends.entry(*a).or_default() += 1;
                *ends.entry(*b).or_default() += 1;
                segs.push((*a, *b));
            }
            Item::Label { at, .. } => *pins.entry(*at).or_default() += 1,
            Item::Symbol { lib_id, at, orient, inst, .. } => {
                let sym = inst.as_ref().map(|i| &d.symbols[d.instances[*i].part]);
                match sym {
                    Some(s) => {
                        for p in &s.pins {
                            *pins.entry(place_pin(p, *orient, *at).at).or_default() += 1;
                        }
                    }
                    None => {
                        let _ = lib_id;
                        *pins.entry(*at).or_default() += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let mut points: Vec<Pt> = ends.keys().chain(pins.keys()).copied().collect();
    points.sort();
    points.dedup();
    let mut out = Vec::new();
    for p in points {
        let through = segs.iter().filter(|(a, b)| *a != p && *b != p && crate::geom::on_segment(p, *a, *b)).count() as u32;
        let deg = ends.get(&p).copied().unwrap_or(0) + pins.get(&p).copied().unwrap_or(0) + 2 * through;
        let has_wire = ends.contains_key(&p) || through > 0;
        if deg >= 3 && has_wire {
            out.push(Item::Junction(p));
        }
    }
    items.extend(out);
}
