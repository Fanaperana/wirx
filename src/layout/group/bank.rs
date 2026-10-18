//! Decoupling banks: capacitors between one supply and one ground drawn side by side, joined by
//! one supply wire and one ground return, each rail with a single power symbol:
//!
//! ```text
//! +3V3 ──┬──────┬────── U2.VCC
//!        │      │
//!      10uF   100nF
//!        │      │
//! GND ───┴──────┴
//! ```
//!
//! A bank is drawn beside the pin it serves when that pin is known: a `near(cap, u.VCC)` hint
//! first, then a chain that wired the capacitor to the pin itself (`u.VCC -> C(..) -> GND`), then
//! the one power pin on that rail in the group. Several candidate pins and no hint: the bank is
//! drawn on its own, never joined to a guessed pin. This is a drawing convention only; it says
//! nothing about where the capacitors go on the board.

use super::*;
use crate::symbol::PinType;

pub(super) struct Bank {
    pub supply: usize,
    pub ground: usize,
    /// Farthest from the pin first: the largest value, so the smallest sits next to the pin.
    pub caps: Vec<usize>,
    /// The pin the bank is drawn at.
    pub pin: Option<(usize, usize)>,
    /// A `near` hint names a pin the bank cannot be drawn at (kept as a plain placement hint).
    pub near_only: bool,
}

/// A capacitor whose pins are on a supply rail and a ground rail: (supply net, ground net).
/// Negative rails stay out: their symbols point down, like ground's, so the bank's shape (supply
/// on top, ground below) does not fit them.
pub(super) fn decap(d: &Design, i: usize) -> Option<(usize, usize)> {
    let inst = &d.instances[i];
    let part = &d.parts[inst.part];
    if !matches!(part.symbol.as_str(), "capacitor" | "cap_polarized") || inst.pin_net.len() != 2 || inst.nc.iter().any(|&x| x) {
        return None;
    }
    let (a, b) = (inst.pin_net[0]?, inst.pin_net[1]?);
    match (d.nets[a].kind, d.nets[b].kind) {
        (NetKind::Rail(RailKind::Supply), NetKind::Rail(RailKind::Ground)) => Some((a, b)),
        (NetKind::Rail(RailKind::Ground), NetKind::Rail(RailKind::Supply)) => Some((b, a)),
        _ => None,
    }
}

/// Capacitance in farads from a value as written (`100nF`, `4.7uF`, `4u7`, `22µF`); 0 if unreadable.
pub(super) fn farads(v: &str) -> f64 {
    let mut int = String::new();
    let mut frac = String::new();
    let mut mult = 1.0;
    let mut after = false;
    for c in v.trim().chars() {
        match c {
            '0'..='9' if after => frac.push(c),
            '0'..='9' => int.push(c),
            '.' if !after => after = true,
            'p' | 'n' | 'u' | 'µ' | 'μ' | 'm' if !int.is_empty() && mult == 1.0 => {
                mult = match c {
                    'p' => 1e-12,
                    'n' => 1e-9,
                    'm' => 1e-3,
                    _ => 1e-6,
                };
                after = true;
            }
            _ => break,
        }
    }
    format!("{}.{}", if int.is_empty() { "0" } else { &int }, if frac.is_empty() { "0" } else { &frac }).parse::<f64>().unwrap_or(0.0) * mult
}

impl Ctx<'_> {
    /// Can a bank on `supply` be drawn at pin `k` of `b`? A visible pin of a chip in this group,
    /// on that rail, with its own power symbol (not shared with neighbouring pins).
    fn bank_pin_ok(&self, b: usize, k: usize, supply: usize) -> bool {
        let d = self.d;
        let inst = &d.instances[b];
        if !self.in_group.contains(&b) || self.class(b) != Class::Fixed || inst.pin_net.get(k).copied().flatten() != Some(supply) || inst.nc[k] || d.pin(b, k).hidden {
            return false;
        }
        let pins: Vec<PlacedPin> = self.sym(b).pins.iter().map(|p| place_pin(p, Orient::IDENTITY, pt(0, 0))).collect();
        !stacked_pins(&pins).contains(&k)
    }

    /// The run of neighbouring pins on one rail (sharing one power symbol) that pin `k` of `b`
    /// belongs to, if any.
    pub(super) fn run_of(&self, b: usize, k: usize) -> Option<Vec<usize>> {
        let pins: Vec<PlacedPin> = self.sym(b).pins.iter().map(|p| place_pin(p, Orient::IDENTITY, pt(0, 0))).collect();
        rail_runs(self.d, b, &pins).into_iter().find(|run| run.contains(&k))
    }

    /// The pin a bank is filed under: a run of pins counts as its first pin.
    fn bank_key(&self, b: usize, k: usize) -> (usize, usize) {
        (b, self.run_of(b, k).map_or(k, |run| run[0]))
    }

    /// Group this group's decoupling capacitors into banks and pick each bank's pin.
    pub(super) fn plan_banks(&self, near: &HashMap<usize, (usize, usize)>) -> Vec<Bank> {
        let d = self.d;
        let group = &d.groups[self.g];
        // Power pins of the group's chips on each supply: the pin a bank serves when it is the only one.
        let power_pins = |supply: usize| -> Vec<(usize, usize)> {
            let mut v = Vec::new();
            for &b in &group.instances {
                if self.class(b) != Class::Fixed {
                    continue;
                }
                for k in 0..d.instances[b].pin_net.len() {
                    let pin = d.pin(b, k);
                    // A run of neighbouring pins on the rail is one place to draw a bank.
                    let key = self.bank_key(b, k);
                    if d.instances[b].pin_net[k] == Some(supply) && !pin.hidden && !d.instances[b].nc[k] && matches!(pin.ptype, PinType::PowerIn | PinType::PowerOut) && !v.contains(&key) {
                        v.push(key);
                    }
                }
            }
            v
        };
        let mut banks: Vec<Bank> = Vec::new();
        for &i in &group.instances {
            let Some((supply, ground)) = decap(d, i) else { continue };
            let (pin, near_only) = if let Some(&(b, k)) = near.get(&i) {
                // An explicit hint is never overridden by a guess.
                if self.bank_pin_ok(b, k, supply) { (Some((b, k)), false) } else { (None, true) }
            } else if let Some(&(_, b, k)) = group.chained.iter().find(|h| h.0 == i && self.bank_pin_ok(h.1, h.2, supply)) {
                (Some((b, k)), false)
            } else {
                match power_pins(supply).as_slice() {
                    [(b, k)] if self.bank_pin_ok(*b, *k, supply) => (Some((*b, *k)), false),
                    _ => (None, false),
                }
            };
            let pin = pin.map(|(b, k)| self.bank_key(b, k));
            match banks.iter_mut().find(|x| x.supply == supply && x.ground == ground && x.pin == pin && !x.near_only && !near_only) {
                Some(x) => x.caps.push(i),
                None => banks.push(Bank { supply, ground, caps: vec![i], pin, near_only }),
            }
        }
        for b in &mut banks {
            b.caps.sort_by(|&x, &y| farads(&d.instances[y].value).total_cmp(&farads(&d.instances[x].value)).then(x.cmp(&y)));
        }
        banks
    }

    /// Lay a bank out around the origin: capacitors standing upright left to right (farthest from
    /// the pin on the left when `far_left`), a supply wire over them and a ground return under
    /// them, each starting at its power symbol beyond the far capacitor. Returns the parts (the
    /// bank's wires and symbols are the first part's decorations) and the supply wire's near end.
    /// With `flag`, the rail's PWR_FLAG continues the supply wire beyond its symbol.
    pub(super) fn bank_unit(&mut self, b: &Bank, far_left: bool, flag: bool) -> (Vec<Placed>, Pt) {
        let d = self.d;
        let mut order = b.caps.clone();
        if !far_left {
            order.reverse();
        }
        let supply_pin = |c: usize| (0..2).find(|&k| d.instances[c].pin_net[k] == Some(b.supply)).unwrap();
        let mut caps: Vec<Placed> = Vec::new();
        let mut right_edge: Option<i32> = None;
        for &c in &order {
            self.bank_pins.insert((c, 0));
            self.bank_pins.insert((c, 1));
            let o = self.rail_orient(c).unwrap_or(Orient::IDENTITY);
            let mut p = self.make_placed(c, o);
            let tip = p.pins[supply_pin(c)].at;
            let ext = p.rects.iter().copied().reduce(Rect::union).unwrap();
            // Each capacitor's text sits on its right: the next one stands clear of it.
            let x = match right_edge {
                Some(r) => (r + CLEAR - (ext.x0 - tip.x) + 99).div_euclid(100) * 100,
                None => 0,
            };
            p.translate(pt(x, 0) - tip);
            right_edge = Some(x + ext.x1 - tip.x);
            caps.push(p);
        }
        let tops: Vec<Pt> = caps.iter().map(|p| p.pins[supply_pin(p.inst)].at).collect();
        let bottoms: Vec<Pt> = caps.iter().map(|p| p.pins[1 - supply_pin(p.inst)].at).collect();
        let (far, near) = if far_left { (tops[0].x, tops.last().unwrap().x) } else { (tops.last().unwrap().x, tops[0].x) };
        // The power symbol's name clears the far capacitor's text (a long rail name needs more room).
        let far_inst = &d.instances[*b.caps.first().unwrap()];
        let longest = [&far_inst.reference, &far_inst.value, &d.nets[b.supply].name, &d.nets[b.ground].name].map(|t| text_width(t, TEXT));
        let room = ((longest.iter().max().unwrap() + 60 + 99) / 100 * 100).max(200);
        let sym_x = if far_left { far - room } else { far + room };
        let bus = -STUB;
        let gbus = bottoms.iter().map(|p| p.y).max().unwrap() + STUB;
        let far_path = &d.instances[*b.caps.first().unwrap()].path;

        let mut wires: Vec<(Pt, Pt)> = tops.iter().map(|&t| (t, pt(t.x, bus))).collect();
        wires.push((pt(sym_x, bus), pt(near, bus)));
        let mut pois: Vec<Pt> = tops.iter().flat_map(|&t| [t, pt(t.x, bus)]).collect();
        pois.push(pt(sym_x, bus));
        let s_kind = match d.nets[b.supply].kind {
            NetKind::Rail(k) => k,
            _ => RailKind::Supply,
        };
        let sup = self.rail_decor(b.supply, s_kind, format!("bank:{far_path}:supply"), wires, pois, pt(sym_x, bus), flag.then_some((pt(sym_x, bus), if far_left { 180 } else { 0 })));

        let (wires, pois, at) = if caps.len() == 1 {
            let g = bottoms[0];
            (vec![(g, pt(g.x, gbus))], vec![g, pt(g.x, gbus)], pt(g.x, gbus))
        } else {
            let mut w: Vec<(Pt, Pt)> = bottoms.iter().map(|&g| (g, pt(g.x, gbus))).collect();
            w.push((pt(sym_x, gbus), pt(near, gbus)));
            let mut p: Vec<Pt> = bottoms.iter().flat_map(|&g| [g, pt(g.x, gbus)]).collect();
            p.push(pt(sym_x, gbus));
            (w, p, pt(sym_x, gbus))
        };
        let g_kind = match d.nets[b.ground].kind {
            NetKind::Rail(k) => k,
            _ => RailKind::Ground,
        };
        let gnd = self.rail_decor(b.ground, g_kind, format!("bank:{far_path}:ground"), wires, pois, at, None);
        for dc in [sup, gnd] {
            caps[0].rects.extend(dc.block.iter().copied());
            caps[0].rects.extend(dc.wires.iter().map(|w| wire_rect(w.0, w.1)));
            caps[0].decor.push(dc);
        }
        (caps, pt(near, bus))
    }

    /// Draw a bank at its pin: the pin's own power symbol gives way to a wire into the bank's supply
    /// wire. Tries growing distances, nearest first, toward the chip's nearer corner (only that way
    /// when `outward_only`, then the other way otherwise); false (nothing changed) when no position
    /// is clear.
    pub(super) fn attach_bank(&mut self, b: &Bank, outward_only: bool, placed: &mut HashMap<usize, Placed>, order: &mut Vec<usize>, occupied: &mut Vec<Rect>) -> bool {
        let Some((chip, k)) = b.pin else { return false };
        let Some(old) = placed.get(&chip).cloned() else { return false };
        // A run of pins keeps its short bus; the bank's wire continues it from one end.
        let run = self.run_of(chip, k).unwrap_or_else(|| vec![k]);
        for &q in &run {
            self.bank_pins.insert((chip, q));
        }
        let mut ic = self.make_placed(chip, old.orient);
        ic.translate(old.origin);
        let mut others: Vec<Rect> = order.iter().filter(|&&i| i != chip).flat_map(|i| placed[i].rects.iter().copied()).chain(ic.rects.iter().copied()).collect();
        // Signal pins already in line will be joined by a straight wire: keep the bank off it.
        let d = self.d;
        let tip = |i: usize, q: usize| if i == chip { ic.pins[q].at } else { placed[&i].pins[q].at };
        for (n, net) in d.nets.iter().enumerate() {
            if net.is_rail() {
                continue;
            }
            let here: Vec<Pt> = net.pins.iter().filter(|&&(i, q)| (i == chip || placed.contains_key(&i)) && !d.instances[i].nc[q] && d.instances[i].pin_net[q] == Some(n)).map(|&(i, q)| tip(i, q)).collect();
            for (x, &a) in here.iter().enumerate() {
                for &z in &here[x + 1..] {
                    if a != z && (a.x == z.x || a.y == z.y) {
                        others.push(wire_rect(a, z));
                    }
                }
            }
        }
        let others = others;
        let t = ic.pins[k];
        // Where the bank's wire starts, for a bank going left (or up a side) and one going right
        // (or down a side): the pin tip, or the end of the run's bus.
        let ends: Vec<Pt> = if run.len() > 1 { run.iter().map(|&q| ic.pins[q].at + dir_vec(t.out) * STUB).collect() } else { vec![t.at] };
        let first = *ends.iter().min_by_key(|p| (p.x, p.y)).unwrap();
        let last = *ends.iter().max_by_key(|p| (p.x, p.y)).unwrap();
        // On a top or bottom edge the bank goes toward the nearer corner of the chip, so banks of
        // neighbouring pins fan out without crossing.
        let toward_left = t.at.x - ic.body.x0 <= ic.body.x1 - t.at.x;
        // (cost, far_left, path from the pin to the bank's near end)
        let mut options: Vec<(i32, bool, Vec<Pt>)> = Vec::new();
        match t.out {
            0 | 180 => {
                for s in (200..=2000).step_by(100) {
                    let n = first + dir_vec(t.out) * s;
                    options.push((s, t.out == 180, vec![first, n]));
                }
            }
            out => {
                for far_left in [true, false] {
                    let start = if far_left { first } else { last };
                    for s in (200..=4000).step_by(100) {
                        for u in (STUB..=5000).step_by(GRID as usize) {
                            let corner = start + dir_vec(out) * u;
                            let n = corner + pt(if far_left { -s } else { s }, 0);
                            options.push((s + u, far_left, vec![start, corner, n]));
                        }
                    }
                }
            }
        }
        if outward_only && !is_horizontal(t.out) {
            options.retain(|o| o.1 == toward_left);
        }
        options.sort_by_key(|o| (o.1 != toward_left, o.0));
        // The pin's PWR_FLAG moves to the bank with its power symbol.
        let flag = run.iter().any(|&q| self.flag_pin.get(&b.supply) == Some(&(chip, q)));
        let units = [self.bank_unit(b, true, flag), self.bank_unit(b, false, flag)];
        // A wire piece, less its ends (which touch the pin and the bank).
        let inner = |a: Pt, z: Pt| -> Option<Rect> {
            let len = (z - a).x.abs() + (z - a).y.abs();
            if len <= 120 {
                return None;
            }
            let v = pt((z - a).x.signum(), (z - a).y.signum());
            Some(Rect::from_points(a + v * 60, z - v * 60).inflate(10))
        };
        // A bank on a top (bottom) pin keeps above (below) the chip's body when it can, so it does
        // not stand beside the side pins, in the way of their wires and labels.
        let beside = |r: Rect| match t.out {
            90 => r.y1 > ic.body.y0 - CLEAR && (r.x0 < ic.body.x0 || r.x1 > ic.body.x1),
            270 => r.y0 < ic.body.y1 + CLEAR && (r.x0 < ic.body.x0 || r.x1 > ic.body.x1),
            _ => false,
        };
        for (allow_beside, (_, far_left, path)) in [false, true].into_iter().flat_map(|a| options.iter().map(move |o| (a, o.clone()))) {
            let (parts, near) = &units[if far_left { 0 } else { 1 }];
            let delta = *path.last().unwrap() - *near;
            if parts.iter().any(|p| collides(p, delta, &others)) {
                continue;
            }
            let bank_rects: Vec<Rect> = parts.iter().flat_map(|p| p.rects.iter().map(|r| r.translate(delta))).collect();
            if !allow_beside && bank_rects.iter().copied().reduce(Rect::union).is_some_and(beside) {
                continue;
            }
            let clear = path.windows(2).filter_map(|w| inner(w[0], w[1])).all(|r| {
                !others.iter().any(|o| r.overlaps(o)) && !bank_rects.iter().any(|o| r.overlaps(o))
            });
            if !clear {
                continue;
            }
            let mut parts = parts.clone();
            for p in &mut parts {
                p.translate(delta);
            }
            let sup = parts[0].decor.iter_mut().find(|dc| dc.net == b.supply as i32).unwrap();
            for w in path.windows(2) {
                sup.wires.push((w[0], w[1]));
                sup.items.push(Item::Wire { a: w[0], b: w[1] });
            }
            sup.pois.extend(path.iter().skip(1).copied());
            let path_rects: Vec<Rect> = path.windows(2).map(|w| wire_rect(w[0], w[1])).collect();
            parts[0].rects.extend(path_rects);
            placed.insert(chip, ic);
            *occupied = others;
            for p in parts {
                occupied.extend(p.rects.iter().copied());
                order.push(p.inst);
                placed.insert(p.inst, p);
            }
            return true;
        }
        for &q in &run {
            self.bank_pins.remove(&(chip, q));
        }
        false
    }

    /// A bank of one capacitor that could not be drawn at its pin goes back to being an ordinary part.
    pub(super) fn unbank(&mut self, c: usize) -> Placed {
        self.bank_pins.remove(&(c, 0));
        self.bank_pins.remove(&(c, 1));
        let o = self.rail_orient(c).unwrap_or(Orient::IDENTITY);
        self.make_placed(c, o)
    }
}

#[cfg(test)]
mod tests {
    use super::farads;

    #[test]
    fn reads_capacitor_values() {
        let close = |v: &str, f: f64| (farads(v) - f).abs() <= f * 1e-9;
        assert!(close("100nF", 100e-9));
        assert!(close("10uF", 10e-6));
        assert!(close("4.7uF", 4.7e-6));
        assert!(close("4u7", 4.7e-6));
        assert!(close("22µF", 22e-6));
        assert!(close("15pF", 15e-12));
        assert_eq!(farads("big"), 0.0);
    }
}
