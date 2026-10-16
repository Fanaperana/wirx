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

/// Free space kept between neighbouring parts (mils).
const CLEAR: i32 = 100;
/// Length of the wire between a pin and the part placed in front of it.
const GAP: i32 = 300;
const STUB: i32 = 100;
const TEXT: i32 = 50;

pub struct GroupOut {
    pub group: usize,
    pub name: String,
    pub items: Vec<Item>,
    pub bbox: Rect,
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
    d: &'a Design,
    g: usize,
    in_group: HashSet<usize>,
    /// Net → the one pin in this group that carries its label.
    label_pin: HashMap<usize, (usize, usize)>,
    /// Net → pin that gets the PWR_FLAG.
    flag_pin: HashMap<usize, (usize, usize)>,
    extra_symbols: &'a mut Vec<SymbolDef>,
    diags: &'a mut Vec<Diagnostic>,
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
fn stub_to(t: Pt, out: Rot, natural: Rot, len: i32) -> (Vec<(Pt, Pt)>, Pt) {
    if out == natural {
        (vec![], t)
    } else if is_horizontal(out) {
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
        let mut in_run: HashSet<usize> = HashSet::new();
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
                let s = if out == kind.natural_dir() {
                    ends[ends.len() / 2]
                } else {
                    // Along the bus to the end nearest the symbol's direction, then one step out.
                    let toward = dir_vec(kind.natural_dir());
                    let tip = if (last - first).x * toward.x + (last - first).y * toward.y >= 0 { last } else { first };
                    let s = tip + toward * STUB;
                    wires.push((tip, s));
                    s
                };
                pois.push(s);
                let flag = run.iter().any(|&k| self.flag_pin.get(&n) == Some(&(inst, k)));
                let key = format!("pwr:{}:{}", i.path, sym.pins[run[0]].number);
                decor.push(self.rail_decor(n, kind, key, wires, pois, s, flag, out));
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
            if let NetKind::Rail(kind) = netd.kind {
                // A symbol drawn up/down from a side pin crosses the neighbouring pin's row: leave that
                // pin room to get out by moving the symbol further away.
                let toward = dir_vec(kind.natural_dir());
                let crowded = pins.iter().enumerate().any(|(j, q)| j != k && q.out == out && q.at == t + toward * 100 && i.pin_net[j] != Some(n));
                let (wires, s) = stub_to(t, out, kind.natural_dir(), if crowded { 250 } else { STUB });
                let flag = self.flag_pin.get(&n) == Some(&(inst, k));
                let key = format!("pwr:{}:{}", i.path, sym.pins[k].number);
                decor.push(self.rail_decor(n, kind, key, wires, vec![t, s], s, flag, out));
            } else if self.label_pin.get(&n) == Some(&(inst, k)) {
                let s = t + dir_vec(out) * STUB;
                let mut dc = Decor { net: n as i32, wires: vec![(t, s)], pois: vec![t, s], ..Default::default() };
                dc.block.push(label_rect(s, out, &netd.name));
                dc.items.push(Item::Wire { a: t, b: s });
                dc.items.push(Item::Label { net: n, text: netd.name.clone(), at: s, rot: out, global: false, shape: "bidirectional" });
                decor.push(dc);
            } else if self.flag_pin.get(&n) == Some(&(inst, k)) {
                let (wires, s) = stub_to(t, out, 90, STUB);
                let mut dc = Decor { net: n as i32, wires: wires.clone(), pois: vec![t, s], ..Default::default() };
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

    /// A power symbol for rail `n` at `s`, reached through `wires`; optionally with a PWR_FLAG.
    #[allow(clippy::too_many_arguments)]
    fn rail_decor(&mut self, n: usize, kind: RailKind, key: String, wires: Vec<(Pt, Pt)>, pois: Vec<Pt>, s: Pt, flag: bool, out: Rot) -> Decor {
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
        if flag {
            let side = if is_horizontal(out) { out } else { 0 };
            let f = s + dir_vec(side) * 200;
            dc.wires.push((s, f));
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
fn rail_runs(d: &Design, inst: usize, pins: &[PlacedPin]) -> Vec<Vec<usize>> {
    let i = &d.instances[inst];
    let rail = |k: usize| i.pin_net[k].filter(|&n| d.nets[n].is_rail() && !i.nc[k]);
    let mut by_side: HashMap<(usize, Rot), Vec<usize>> = HashMap::new();
    for k in 0..pins.len() {
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

pub fn layout_group(d: &Design, g: usize, extra_symbols: &mut Vec<SymbolDef>, flags_done: &mut HashSet<usize>, diags: &mut Vec<Diagnostic>) -> GroupOut {
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
        if !net.is_rail() && here.len() < net.pins.len() {
            let pick = here.iter().find(|p| fixed(p)).or(here.first()).copied().unwrap();
            label_pin.insert(n, pick);
        }
        if net.flagged && !flags_done.contains(&n) {
            let candidates: Vec<&(usize, usize)> = here.iter().filter(|p| label_pin.get(&n) != Some(p) && !d.instances[p.0].nc[p.1]).collect();
            if let Some(p) = candidates.first() {
                flag_pin.insert(n, **p);
                flags_done.insert(n);
            }
        }
    }

    let mut ctx = Ctx { d, g, in_group, label_pin, flag_pin, extra_symbols, diags };
    let mut placed: HashMap<usize, Placed> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    let mut occupied: Vec<Rect> = Vec::new();

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

    // Clusters of signal-connected parts, each grown from its biggest part.
    loop {
        let anchor = group
            .instances
            .iter()
            .copied()
            .filter(|i| !placed.contains_key(i) && !rail_only(*i))
            .max_by_key(|&i| (d.parts[d.instances[i].part].pins.len(), std::cmp::Reverse(i)));
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
                (side, if is_horizontal(pp[k].out) { -pp[k].at.y } else { pp[k].at.x })
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
                    commit(p, &mut placed, &mut order, &mut occupied);
                    if straight {
                        occupied.push(wire_rect(anchor_pin, tip));
                    }
                }
            }
        }
    }

    // Rail-only parts (decoupling, bulk caps): a `near` hint puts one at its pin; the rest stand in
    // one tidy row under the circuit, supplies on top and grounds on one baseline.
    let mut row: Vec<Placed> = Vec::new();
    for &i in &group.instances {
        if placed.contains_key(&i) {
            continue;
        }
        let orient = ctx.rail_orient(i).unwrap_or(Orient::IDENTITY);
        let proto = ctx.make_placed(i, orient);
        if let Some(t) = near.get(&i).and_then(|&(b, k)| placed.get(&b).map(|p| p.pins[k])) {
            let ext = proto.rects.iter().copied().reduce(Rect::union).unwrap();
            let desired = t.at + dir_vec(t.out) * 300 - pt(ext.center().x, 0);
            let p = place_near(&proto, pt(snap50(desired.x), snap50(desired.y)), &occupied);
            commit(p, &mut placed, &mut order, &mut occupied);
        } else {
            row.push(proto);
        }
    }
    tidy(&ctx, &mut placed, &order);
    let mut occupied: Vec<Rect> = order.iter().flat_map(|i| placed[i].rects.iter().copied()).collect();
    // A group made only of rail-connected parts (an LDO between two rails): its biggest part
    // anchors it, the rest go beside the pins they serve.
    if order.is_empty() && !row.is_empty() {
        let big = (0..row.len()).max_by_key(|&k| (row[k].pins.len(), std::cmp::Reverse(row[k].inst))).unwrap();
        let p = row.remove(big);
        commit(p, &mut placed, &mut order, &mut occupied);
    }
    if !row.is_empty() {
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
        let mut sides: Vec<(Option<Rot>, Option<PlacedPin>, Vec<Placed>)> = Vec::new();
        for p in row {
            let pin = served(&p, &placed);
            let side = side_of(pin);
            match sides.iter_mut().find(|(s, _, _)| *s == side) {
                Some((_, _, list)) => list.push(p),
                None => sides.push((side, pin, vec![p])),
            }
        }
        for (side, pin, mut list) in sides {
            let rail_name = |p: &Placed| {
                let inst = &d.instances[p.inst];
                inst.pin_net.iter().flatten().map(|&n| &d.nets[n]).find(|n| matches!(n.kind, NetKind::Rail(RailKind::Supply))).map(|n| n.name.clone()).unwrap_or_default()
            };
            list.sort_by_key(|p| (rail_name(p), p.inst));
            let mut x = 0;
            for p in &mut list {
                let ext = p.rects.iter().copied().reduce(Rect::union).unwrap();
                p.translate(pt(snap50(x - ext.x0), 0));
                x = p.rects.iter().copied().reduce(Rect::union).unwrap().x1 + 2 * CLEAR + 50;
            }
            let row_ext = list.iter().flat_map(|p| p.rects.iter().copied()).reduce(Rect::union).unwrap();
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
            let fits = |d: Pt, occupied: &[Rect]| list.iter().all(|p| !collides(p, d, occupied));
            let shift = ring_offsets().iter().map(|&o| base + o).find(|&d| fits(d, &occupied)).unwrap_or(base);
            for mut p in list {
                p.translate(shift);
                commit(p, &mut placed, &mut order, &mut occupied);
            }
        }
    }

    let occupied: Vec<Rect> = order.iter().flat_map(|i| placed[i].rects.iter().copied()).collect();
    route_group(&mut ctx, &placed, &order, &occupied)
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
    let target = mid + dir_vec(out) * (GAP + depth) - pins_mid;
    let base = pt(snap50(target.x), snap50(target.y));
    let along = dir_vec(out);
    let across = dir_vec(crate::geom::norm_rot(out as i32 - 90));
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
        at + dir_vec(out) * GAP
    } else if is_horizontal(q.out) != is_horizontal(out) {
        at + dir_vec(out) * GAP + dir_vec(opposite(q.out)) * 100
    } else {
        at + dir_vec(out) * (GAP * 2)
    };
    let base = tip - q.at;
    let along = dir_vec(out);
    let across = dir_vec(crate::geom::norm_rot(out as i32 - 90));
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
    let span = |terms: &[(usize, usize)]| bounds(terms.iter().map(|t| tip(t).at)).map(|r| r.w() + r.h()).unwrap_or(0);
    let mut net_order: Vec<usize> = (0..nets.len()).collect();
    net_order.sort_by_key(|&i| (span(&nets[i].1), nets[i].0));
    type Attempt = (Vec<(Pt, Pt)>, Vec<(usize, (usize, usize))>);
    let mut best: Option<Attempt> = None;
    for _attempt in 0..4 {
        let mut g = grid.clone();
        let mut wires = Vec::new();
        let mut failed = Vec::new();
        for &ni in &net_order {
            let (n, terms) = (&nets[ni].0, &nets[ni].1);
            let n = *n;
            let first = ctx.label_pin.get(&n).copied().filter(|p| terms.contains(p)).unwrap_or(terms[0]);
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
                let (ri, _) = rest
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, t)| tree_pts.iter().map(|q| q.manhattan(tip(t).at)).min().unwrap_or(0))
                    .unwrap();
                let t = rest.remove(ri);
                let tp = tip(&t);
                if g.idx(tp.at).is_some_and(|c| tree.contains(&c)) {
                    continue;
                }
                match g.route(n as i32, tp.at, tp.out, &tree) {
                    Some(path) => {
                        for w in path.windows(2) {
                            g.mark_wire(w[0], w[1], n as i32);
                            tree.extend(g.segment_cells(w[0], w[1]));
                            wires.push((w[0], w[1]));
                        }
                        tree_pts.extend(path.iter().copied());
                    }
                    None => failed.push((n, t)),
                }
            }
        }
        let done = failed.is_empty();
        if best.as_ref().is_none_or(|b| failed.len() < b.1.len()) {
            best = Some((wires, failed.clone()));
        }
        if done {
            break;
        }
        let failing: HashSet<usize> = failed.iter().map(|f| f.0).collect();
        net_order.sort_by_key(|&i| !failing.contains(&nets[i].0));
    }
    let (wires, failed) = best.unwrap_or_default();
    let mut extra_labels: Vec<Item> = Vec::new();
    let mut labeled: HashSet<usize> = ctx.label_pin.keys().copied().collect();
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
    GroupOut { group: ctx.g, name: group.name.clone(), items, bbox: bb.unwrap_or(Rect::new(0, 0, 100, 100)) }
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
