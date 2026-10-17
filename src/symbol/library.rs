//! Symbol libraries: read symbols from eleceda (`.elib`, or the symbols embedded in an `.esch`) and
//! KiCad (`.kicad_sym`) files, and write wirx symbols back out as either, so a symbol defined in
//! wirx resolves in eleceda and KiCad without redrawing it.

use super::{Field, Fill, Graphic, Justify, Pin, PinType, SymbolDef};
use crate::geom::{pt, Pt, Rot};
use serde_json::Value;
use std::fmt::Write;
use std::path::Path;

/// Every symbol in a library file (by extension: `.elib`, `.esch`, `.kicad_sym`).
pub fn load(path: &Path) -> Result<Vec<SymbolDef>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "elib" | "esch" | "json" => parse_eleceda(&text).map_err(|e| format!("{}: {e}", path.display())),
        "kicad_sym" => parse_kicad(&text).map_err(|e| format!("{}: {e}", path.display())),
        _ => Err(format!("{}: unknown library type (use .elib, .esch or .kicad_sym)", path.display())),
    }
}

/// Find one symbol by name (`RP2350B`) or full id (`MCU_RaspberryPi:RP2350B`).
pub fn find<'a>(symbols: &'a [SymbolDef], name: &str) -> Option<&'a SymbolDef> {
    symbols.iter().find(|s| s.id == name).or_else(|| symbols.iter().find(|s| s.name == name || s.id.rsplit(':').next() == Some(name)))
}

// ------------------------------------------------------------------ eleceda JSON

/// eleceda stores coordinates as JSON numbers that may be fractional; wirx works on whole mils.
fn round_numbers(v: &mut Value) {
    match v {
        Value::Number(n) if n.is_f64() => *v = Value::from(n.as_f64().unwrap().round() as i64),
        Value::Array(a) => a.iter_mut().for_each(round_numbers),
        Value::Object(o) => o.values_mut().for_each(round_numbers),
        _ => {}
    }
}

fn parse_eleceda(text: &str) -> Result<Vec<SymbolDef>, String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    round_numbers(&mut v);
    let list: Vec<Value> = match &v["symbols"] {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.values().cloned().collect(),
        _ => return Err("no `symbols` in this file".into()),
    };
    let mut out = Vec::new();
    for s in list {
        let id = s["id"].as_str().unwrap_or("?").to_string();
        let mut def: SymbolDef = serde_json::from_value(s).map_err(|e| format!("symbol {id}: {e}"))?;
        // Keep only what wirx draws: unit 1 (or shared) and the normal body style.
        def.pins.retain(|p| p.unit <= 1);
        out.push(def);
    }
    Ok(out)
}

/// An eleceda library file (`.elib`) holding `symbols`.
pub fn write_elib(name: &str, symbols: &[SymbolDef]) -> String {
    let v = serde_json::json!({ "format": "eleceda-library", "version": 1, "name": name, "symbols": symbols });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

// ------------------------------------------------------------------ KiCad

#[derive(Debug, Clone)]
enum Sx {
    Atom(String),
    List(Vec<Sx>),
}

impl Sx {
    fn head(&self) -> Option<&str> {
        match self {
            Sx::List(v) => match v.first() {
                Some(Sx::Atom(a)) => Some(a),
                _ => None,
            },
            _ => None,
        }
    }
    fn items(&self) -> &[Sx] {
        match self {
            Sx::List(v) => v,
            _ => &[],
        }
    }
    fn atom(&self, i: usize) -> Option<&str> {
        match self.items().get(i) {
            Some(Sx::Atom(a)) => Some(a),
            _ => None,
        }
    }
    fn num(&self, i: usize) -> f64 {
        self.atom(i).and_then(|a| a.parse().ok()).unwrap_or(0.0)
    }
    fn child(&self, name: &str) -> Option<&Sx> {
        self.items().iter().find(|c| c.head() == Some(name))
    }
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sx> + 'a {
        self.items().iter().filter(move |c| c.head() == Some(name))
    }
}

fn parse_sx(text: &str) -> Result<Sx, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    fn value(chars: &[char], i: &mut usize) -> Result<Sx, String> {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
        match chars.get(*i) {
            None => Err("unexpected end of file".into()),
            Some('(') => {
                *i += 1;
                let mut list = Vec::new();
                loop {
                    while *i < chars.len() && chars[*i].is_whitespace() {
                        *i += 1;
                    }
                    match chars.get(*i) {
                        None => return Err("unclosed `(`".into()),
                        Some(')') => {
                            *i += 1;
                            return Ok(Sx::List(list));
                        }
                        _ => list.push(value(chars, i)?),
                    }
                }
            }
            Some('"') => {
                *i += 1;
                let mut s = String::new();
                while let Some(&c) = chars.get(*i) {
                    *i += 1;
                    match c {
                        '"' => return Ok(Sx::Atom(s)),
                        '\\' => {
                            if let Some(&n) = chars.get(*i) {
                                s.push(if n == 'n' { '\n' } else { n });
                                *i += 1;
                            }
                        }
                        _ => s.push(c),
                    }
                }
                Err("unclosed string".into())
            }
            Some(_) => {
                let start = *i;
                while *i < chars.len() && !chars[*i].is_whitespace() && chars[*i] != '(' && chars[*i] != ')' {
                    *i += 1;
                }
                Ok(Sx::Atom(chars[start..*i].iter().collect()))
            }
        }
    }
    value(&chars, &mut i)
}

const MIL_PER_MM: f64 = 1.0 / 0.0254;

/// KiCad millimetres, Y up → mils, Y down.
fn kpt(x: f64, y: f64) -> Pt {
    pt((x * MIL_PER_MM).round() as i32, (-y * MIL_PER_MM).round() as i32)
}

fn kmil(v: f64) -> i32 {
    (v * MIL_PER_MM).round() as i32
}

fn kicad_pin_type(t: &str) -> PinType {
    match t {
        "input" => PinType::Input,
        "output" => PinType::Output,
        "bidirectional" => PinType::Bidirectional,
        "tri_state" => PinType::TriState,
        "passive" => PinType::Passive,
        "free" => PinType::Free,
        "power_in" => PinType::PowerIn,
        "power_out" => PinType::PowerOut,
        "open_collector" => PinType::OpenCollector,
        "open_emitter" => PinType::OpenEmitter,
        "no_connect" => PinType::NoConnect,
        _ => PinType::Unspecified,
    }
}

fn kicad_fill(s: &Sx) -> Option<Fill> {
    match s.child("fill").and_then(|f| f.child("type")).and_then(|t| t.atom(1)) {
        Some("background") => Some(Fill::Background),
        Some("outline") => Some(Fill::Outline),
        _ => Some(Fill::None),
    }
}

/// Unit and body style from a sub-symbol name `NAME_U_S`.
fn unit_style(name: &str) -> (u32, u32) {
    let mut parts = name.rsplitn(3, '_');
    let style = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let unit = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (unit, style)
}

fn parse_kicad(text: &str) -> Result<Vec<SymbolDef>, String> {
    let root = parse_sx(text)?;
    if root.head() != Some("kicad_symbol_lib") {
        return Err("not a KiCad symbol library".into());
    }
    let mut out: Vec<SymbolDef> = Vec::new();
    let raw: Vec<&Sx> = root.children("symbol").collect();
    for s in &raw {
        let name = s.atom(1).unwrap_or("?").to_string();
        // Derived symbols (`extends`) reuse their parent's drawing and pins.
        let base = match s.child("extends").and_then(|e| e.atom(1)) {
            Some(parent) => raw.iter().find(|r| r.atom(1) == Some(parent)).copied().unwrap_or(s),
            None => s,
        };
        let prop = |key: &str| -> String {
            s.children("property").chain(base.children("property")).find(|p| p.atom(1) == Some(key)).and_then(|p| p.atom(2)).unwrap_or("").to_string()
        };
        let mut def = SymbolDef {
            id: format!("kicad:{name}"),
            name: name.clone(),
            description: Some(prop("Description")).filter(|d| !d.is_empty()),
            keywords: Some(prop("ki_keywords")).filter(|d| !d.is_empty()),
            ref_prefix: Some(prop("Reference")).filter(|d| !d.is_empty()).unwrap_or_else(|| "U".into()),
            unit_count: 1,
            power: s.child("power").or(base.child("power")).map(|_| true),
            hide_pin_names: base.child("pin_names").and_then(|p| p.items().iter().find(|c| c.head() == Some("hide") || matches!(c, Sx::Atom(a) if a == "hide"))).map(|_| true),
            hide_pin_numbers: base.child("pin_numbers").and_then(|p| p.items().iter().find(|c| c.head() == Some("hide") || matches!(c, Sx::Atom(a) if a == "hide"))).map(|_| true),
            pin_name_offset: base.child("pin_names").and_then(|p| p.child("offset")).map(|o| kmil(o.num(1))),
            open_pins_ok: None,
            graphics: vec![],
            pins: vec![],
            fields: vec![
                Field::new("Reference", &prop("Reference"), pt(0, -150), true, Justify::Center),
                Field::new("Value", &name, pt(0, 150), true, Justify::Center),
                Field::new("Footprint", &prop("Footprint"), pt(0, 0), false, Justify::Center),
                Field::new("Datasheet", &prop("Datasheet"), pt(0, 0), false, Justify::Center),
            ],
        };
        let mut max_unit = 1;
        for sub in base.children("symbol") {
            let (unit, style) = unit_style(sub.atom(1).unwrap_or(""));
            max_unit = max_unit.max(unit);
            if style > 1 || unit > 1 {
                continue; // wirx draws unit 1, normal body style
            }
            for g in sub.items() {
                let width = g.child("stroke").and_then(|st| st.child("width")).map(|w| kmil(w.num(1))).filter(|w| *w > 0);
                match g.head() {
                    Some("rectangle") => {
                        let (a, b) = (g.child("start").unwrap(), g.child("end").unwrap());
                        def.graphics.push(Graphic::Rect { start: kpt(a.num(1), a.num(2)), end: kpt(b.num(1), b.num(2)), unit: 0, width, fill: kicad_fill(g) });
                    }
                    Some("polyline") => {
                        let points = g.child("pts").map(|p| p.children("xy").map(|xy| kpt(xy.num(1), xy.num(2))).collect()).unwrap_or_default();
                        def.graphics.push(Graphic::Polyline { points, unit: 0, width, fill: kicad_fill(g) });
                    }
                    Some("circle") => {
                        let c = g.child("center").unwrap();
                        def.graphics.push(Graphic::Circle { center: kpt(c.num(1), c.num(2)), radius: kmil(g.child("radius").map(|r| r.num(1)).unwrap_or(0.0)), unit: 0, width, fill: kicad_fill(g) });
                    }
                    Some("arc") => {
                        let (a, m, b) = (g.child("start").unwrap(), g.child("mid").unwrap(), g.child("end").unwrap());
                        def.graphics.push(Graphic::Arc {
                            start: kpt(a.num(1), a.num(2)),
                            mid: kpt(m.num(1), m.num(2)),
                            end: kpt(b.num(1), b.num(2)),
                            unit: 0,
                            width,
                            fill: Some(Fill::None),
                        });
                    }
                    Some("text") => {
                        let at = g.child("at").unwrap();
                        def.graphics.push(Graphic::Text { at: kpt(at.num(1), at.num(2)), text: g.atom(1).unwrap_or("").into(), unit: 0, size: Some(50) });
                    }
                    Some("pin") => {
                        let at = g.child("at").unwrap();
                        let dir = (at.num(3).round() as i32).rem_euclid(360) as Rot;
                        let hidden = g.items().iter().any(|c| matches!(c, Sx::Atom(a) if a == "hide")) || g.child("hide").and_then(|h| h.atom(1)) == Some("yes");
                        def.pins.push(Pin {
                            at: kpt(at.num(1), at.num(2)),
                            dir,
                            length: kmil(g.child("length").map(|l| l.num(1)).unwrap_or(2.54)),
                            number: g.child("number").and_then(|n| n.atom(1)).unwrap_or("").into(),
                            name: g.child("name").and_then(|n| n.atom(1)).unwrap_or("").into(),
                            ptype: kicad_pin_type(g.atom(1).unwrap_or("")),
                            shape: g.atom(2).unwrap_or("line").into(),
                            unit: 0,
                            hidden: hidden.then_some(true),
                        });
                    }
                    _ => {}
                }
            }
        }
        def.unit_count = max_unit;
        out.push(def);
    }
    Ok(out)
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn mm(v: i32) -> String {
    let s = format!("{:.4}", v as f64 * 0.0254);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".into() } else { s }
}

fn kxy(p: Pt) -> String {
    format!("{} {}", mm(p.x), mm(-p.y))
}

/// A KiCad 6+ symbol library holding `symbols`.
pub fn write_kicad_sym(symbols: &[SymbolDef]) -> String {
    let mut out = String::from("(kicad_symbol_lib (version 20231120) (generator \"wirx\")\n");
    for s in symbols {
        let field = |name: &str| s.fields.iter().find(|f| f.name == name).map(|f| f.value.clone()).unwrap_or_default();
        let _ = writeln!(out, "  (symbol {}{} (in_bom yes) (on_board yes)", q(&s.name), if s.is_power() { " (power)" } else { "" });
        if s.hide_pin_numbers == Some(true) {
            out.push_str("    (pin_numbers hide)\n");
        }
        match (s.hide_pin_names == Some(true), s.pin_name_offset) {
            (true, _) => out.push_str("    (pin_names hide)\n"),
            (false, Some(o)) => {
                let _ = writeln!(out, "    (pin_names (offset {}))", mm(o));
            }
            _ => {}
        }
        let ext = s.extent();
        let props = [
            ("Reference", s.ref_prefix.clone(), pt(ext.x0, ext.y0 - 100), false),
            ("Value", field("Value").is_empty().then(|| s.name.clone()).unwrap_or_else(|| field("Value")), pt(ext.x0, ext.y1 + 100), false),
            ("Footprint", field("Footprint"), pt(0, 0), true),
            ("Datasheet", field("Datasheet"), pt(0, 0), true),
            ("Description", s.description.clone().unwrap_or_default(), pt(0, 0), true),
        ];
        for (k, v, at, hide) in props {
            let _ = writeln!(out, "    (property {} {} (at {} 0) (effects (font (size 1.27 1.27)){}))", q(k), q(&v), kxy(at), if hide { " hide" } else { "" });
        }
        let _ = writeln!(out, "    (symbol {}", q(&format!("{}_0_1", s.name)));
        for g in &s.graphics {
            let fill = |f: &Option<Fill>| match f {
                Some(Fill::Background) => "background",
                Some(Fill::Outline) => "outline",
                _ => "none",
            };
            let stroke = |w: &Option<i32>| format!("(stroke (width {}) (type default))", mm(w.unwrap_or(0)));
            match g {
                Graphic::Rect { start, end, width, fill: f, .. } => {
                    let _ = writeln!(out, "      (rectangle (start {}) (end {}) {} (fill (type {})))", kxy(*start), kxy(*end), stroke(width), fill(f));
                }
                Graphic::Polyline { points, width, fill: f, .. } => {
                    let pts: Vec<String> = points.iter().map(|p| format!("(xy {})", kxy(*p))).collect();
                    let _ = writeln!(out, "      (polyline (pts {}) {} (fill (type {})))", pts.join(" "), stroke(width), fill(f));
                }
                Graphic::Circle { center, radius, width, fill: f, .. } => {
                    let _ = writeln!(out, "      (circle (center {}) (radius {}) {} (fill (type {})))", kxy(*center), mm(*radius), stroke(width), fill(f));
                }
                Graphic::Arc { start, mid, end, width, .. } => {
                    let _ = writeln!(out, "      (arc (start {}) (mid {}) (end {}) {} (fill (type none)))", kxy(*start), kxy(*mid), kxy(*end), stroke(width));
                }
                Graphic::Text { at, text, .. } => {
                    let _ = writeln!(out, "      (text {} (at {} 0) (effects (font (size 1.27 1.27))))", q(text), kxy(*at));
                }
            }
        }
        out.push_str("    )\n");
        let _ = writeln!(out, "    (symbol {}", q(&format!("{}_1_1", s.name)));
        for p in &s.pins {
            let _ = writeln!(
                out,
                "      (pin {} {} (at {} {}) (length {}){} (name {} (effects (font (size 1.27 1.27)))) (number {} (effects (font (size 1.27 1.27)))))",
                p.ptype.name(),
                p.shape,
                kxy(p.at),
                p.dir,
                mm(p.length),
                if p.hidden == Some(true) { " hide" } else { "" },
                q(&p.name),
                q(&p.number)
            );
        }
        out.push_str("    )\n  )\n");
    }
    out.push_str(")\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicad_round_trip_keeps_pins() {
        let sym = crate::symbol::boxgen::box_symbol(
            "wirx:X",
            "X",
            "U",
            &[
                crate::symbol::boxgen::BoxPin { number: "1".into(), name: "IN".into(), ptype: PinType::Input, side: None, hidden: false },
                crate::symbol::boxgen::BoxPin { number: "2".into(), name: "OUT".into(), ptype: PinType::Output, side: None, hidden: false },
            ],
        );
        let text = write_kicad_sym(std::slice::from_ref(&sym));
        let back = parse_kicad(&text).unwrap();
        assert_eq!(back.len(), 1);
        let (a, b) = (&sym.pins, &back[0].pins);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!((x.at, x.dir, x.number.as_str(), x.ptype), (y.at, y.dir, y.number.as_str(), y.ptype));
        }
    }
}
