//! Self-check of the drawing: rebuilds connectivity from geometry the way eleceda does (points on
//! wires join them, labels join by name, power symbols by value) and compares it with the design.
//! A mismatch means a layout bug, never a user error, so `build` refuses to write such a file.

use crate::geom::{on_segment, Pt, Rect};
use crate::ir::Design;
use crate::layout::{Item, Schematic};
use crate::symbol::place_pin;
use std::collections::HashMap;

#[derive(Default)]
struct Uf {
    parent: HashMap<String, String>,
}

impl Uf {
    fn find(&mut self, a: &str) -> String {
        let mut cur = a.to_string();
        loop {
            let p = self.parent.entry(cur.clone()).or_insert_with(|| cur.clone()).clone();
            if p == cur {
                return cur;
            }
            cur = p;
        }
    }
    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }
}

pub fn check(d: &Design, s: &Schematic) -> Vec<String> {
    let mut errors = Vec::new();
    let mut uf = Uf::default();
    let mut pin_key: HashMap<(usize, usize), String> = HashMap::new();
    let sym_of = |id: &str| s.symbols.iter().find(|x| x.id == id);

    for (si, sheet) in s.sheets.iter().enumerate() {
        let key = |p: Pt| format!("{si}|{},{}", p.x, p.y);
        let wires: Vec<(Pt, Pt)> = sheet.items.iter().filter_map(|it| if let Item::Wire { a, b } = it { Some((*a, *b)) } else { None }).collect();
        let mut pois: Vec<Pt> = Vec::new();
        let mut endpoints: HashMap<Pt, u32> = HashMap::new();
        let mut anchors: Vec<Pt> = Vec::new();
        for it in &sheet.items {
            match it {
                Item::Wire { a, b } => {
                    if a.x != b.x && a.y != b.y {
                        errors.push(format!("diagonal wire on {}", sheet.name));
                    }
                    uf.union(&key(*a), &key(*b));
                    pois.extend([*a, *b]);
                    *endpoints.entry(*a).or_default() += 1;
                    *endpoints.entry(*b).or_default() += 1;
                }
                Item::Junction(p) => {
                    pois.push(*p);
                    anchors.push(*p);
                }
                Item::Label { text, at, global, .. } => {
                    let scope = if *global { format!("G:{text}") } else { format!("L:{si}:{text}") };
                    uf.union(&key(*at), &scope);
                    pois.push(*at);
                    anchors.push(*at);
                }
                Item::Symbol { lib_id, at, orient, fields, inst, .. } => {
                    let Some(def) = sym_of(lib_id) else {
                        errors.push(format!("symbol {lib_id} is not embedded"));
                        continue;
                    };
                    for pin in &def.pins {
                        let pp = place_pin(pin, *orient, *at);
                        if pp.at.x % 25 != 0 || pp.at.y % 25 != 0 {
                            errors.push(format!("off-grid pin at {:?}", pp.at));
                        }
                        pois.push(pp.at);
                        anchors.push(pp.at);
                        match inst {
                            Some(i) => {
                                // Match by pin number, not position in the symbol.
                                let part = &d.parts[d.instances[*i].part];
                                match part.pins.iter().position(|p| p.number == pin.number) {
                                    Some(pk) => {
                                        pin_key.insert((*i, pk), key(pp.at));
                                    }
                                    None => errors.push(format!("symbol {lib_id} has pin {} that the part lacks", pin.number)),
                                }
                            }
                            None if def.is_power() && pin.ptype == crate::symbol::PinType::PowerIn => {
                                let v = fields.iter().find(|f| f.name == "Value").map(|f| f.value.clone()).unwrap_or_default();
                                uf.union(&key(pp.at), &format!("G:{v}"));
                            }
                            None => {
                                uf.find(&key(pp.at));
                            }
                        }
                    }
                }
                Item::NoConnect(p) => anchors.push(*p),
                Item::Block { .. } => {}
            }
        }
        for p in &pois {
            for (a, b) in &wires {
                if on_segment(*p, *a, *b) {
                    uf.union(&key(*p), &key(*a));
                }
            }
        }
        // Every wire end must touch something.
        for (a, b) in &wires {
            for e in [a, b] {
                let touches = anchors.contains(e)
                    || endpoints.get(e).copied().unwrap_or(0) > 1
                    || wires.iter().any(|(c, d)| (c, d) != (a, b) && on_segment(*e, *c, *d));
                if !touches {
                    errors.push(format!("dangling wire end at ({}, {}) on {}", e.x, e.y, sheet.name));
                }
            }
        }
        // Every connection is drawn: two symbols' pins may not touch without a wire between them.
        // (That does connect, but reads as a gap and breaks as soon as one part is moved.)
        let mut pin_owner: HashMap<Pt, usize> = HashMap::new();
        for (idx, it) in sheet.items.iter().enumerate() {
            let Item::Symbol { lib_id, at, orient, fields, .. } = it else { continue };
            let Some(def) = sym_of(lib_id) else { continue };
            for pin in &def.pins {
                let p = place_pin(pin, *orient, *at).at;
                if let Some(&other) = pin_owner.get(&p)
                    && other != idx
                {
                    // Power symbols and flags are named by their net (their references are not numbered yet).
                    let label = |fields: &Vec<crate::symbol::Field>| {
                        let get = |k: &str| fields.iter().find(|f| f.name == k).map(|f| f.value.clone()).unwrap_or_default();
                        let r = get("Reference");
                        if r.starts_with('#') { format!("{} symbol", get("Value")) } else { r }
                    };
                    let name = |i: usize| match &sheet.items[i] {
                        Item::Symbol { fields, .. } => label(fields),
                        _ => String::new(),
                    };
                    let me = label(fields);
                    errors.push(format!("{} and {} touch at ({}, {}) on {} with no wire between them", name(other), me, p.x, p.y, sheet.name));
                }
                pin_owner.insert(p, idx);
            }
        }
        // Part bodies must not overlap.
        let bodies: Vec<(usize, Rect)> = sheet
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Symbol { inst: Some(i), lib_id, at, orient, .. } => sym_of(lib_id).map(|def| (*i, orient.rect(def.body()).translate(*at))),
                _ => None,
            })
            .collect();
        for a in 0..bodies.len() {
            for b in a + 1..bodies.len() {
                if bodies[a].1.inflate(-40).overlaps(&bodies[b].1.inflate(-40)) {
                    errors.push(format!("{} overlaps {}", d.instances[bodies[a].0].reference, d.instances[bodies[b].0].reference));
                }
            }
        }
    }

    // Compare with the design's nets.
    let mut root_net: HashMap<String, usize> = HashMap::new();
    for (n, net) in d.nets.iter().enumerate() {
        let mut first: Option<String> = None;
        for &(i, k) in &net.pins {
            let Some(pk) = pin_key.get(&(i, k)) else {
                errors.push(format!("{} is not drawn", d.pin_label(i, k)));
                continue;
            };
            let r = uf.find(pk);
            match &first {
                None => first = Some(r.clone()),
                Some(f) if *f != r => {
                    errors.push(format!("net `{}` is split in the drawing at {}", net.name, d.pin_label(i, k)));
                    break;
                }
                _ => {}
            }
        }
        if net.pins.len() < 2 && !net.is_rail() {
            continue;
        }
        if let Some(r) = first {
            if let Some(&other) = root_net.get(&r) {
                errors.push(format!("nets `{}` and `{}` are shorted in the drawing", d.nets[other].name, net.name));
            } else {
                root_net.insert(r, n);
            }
        }
    }
    errors
}
