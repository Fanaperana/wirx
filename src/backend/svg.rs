//! SVG preview of the laid-out sheets, for quick review by people and AI without opening eleceda.

use crate::geom::{dir_vec, pt, text_width, Orient, Pt};
use crate::layout::{Item, Schematic, SheetOut};
use crate::symbol::{Graphic, Justify, SymbolDef};
use std::fmt::Write;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn tf(o: Orient, at: Pt, p: Pt) -> Pt {
    o.apply(p) + at
}

fn symbol(out: &mut String, def: &SymbolDef, at: Pt, o: Orient) {
    for g in &def.graphics {
        let fill = |f: &Option<crate::symbol::Fill>| match f {
            Some(crate::symbol::Fill::Background) => "#fffbe6",
            Some(crate::symbol::Fill::Outline) => "#7a1010",
            _ => "none",
        };
        match g {
            Graphic::Polyline { points, fill: f, .. } => {
                let pts: Vec<String> = points.iter().map(|p| tf(o, at, *p)).map(|p| format!("{},{}", p.x, p.y)).collect();
                let _ = writeln!(out, r#"<polyline points="{}" fill="{}" stroke="maroon" stroke-width="10"/>"#, pts.join(" "), fill(f));
            }
            Graphic::Rect { start, end, fill: f, .. } => {
                let (a, b) = (tf(o, at, *start), tf(o, at, *end));
                let _ = writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="maroon" stroke-width="10"/>"#,
                    a.x.min(b.x),
                    a.y.min(b.y),
                    (a.x - b.x).abs(),
                    (a.y - b.y).abs(),
                    fill(f)
                );
            }
            Graphic::Circle { center, radius, .. } => {
                let c = tf(o, at, *center);
                let _ = writeln!(out, r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="maroon" stroke-width="10"/>"#, c.x, c.y, radius);
            }
            Graphic::Arc { start, mid, end, .. } => {
                let (a, m, b) = (tf(o, at, *start), tf(o, at, *mid), tf(o, at, *end));
                let _ = writeln!(out, r#"<path d="M {} {} Q {} {} {} {}" fill="none" stroke="maroon" stroke-width="10"/>"#, a.x, a.y, 2 * m.x - (a.x + b.x) / 2, 2 * m.y - (a.y + b.y) / 2, b.x, b.y);
            }
            Graphic::Text { at: p, text, .. } => {
                let c = tf(o, at, *p);
                let _ = writeln!(out, r#"<text x="{}" y="{}" font-size="50" text-anchor="middle">{}</text>"#, c.x, c.y, esc(text));
            }
        }
    }
    for pin in &def.pins {
        if pin.hidden == Some(true) || pin.length == 0 {
            continue;
        }
        let a = tf(o, at, pin.at);
        let b = a + dir_vec(o.angle(pin.dir)) * pin.length;
        let _ = writeln!(out, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="maroon" stroke-width="8"/>"#, a.x, a.y, b.x, b.y);
        if def.hide_pin_names != Some(true) && pin.name != "~" && !pin.name.is_empty() {
            let inside = b + dir_vec(o.angle(pin.dir)) * 30;
            let anchor = match o.angle(pin.dir) {
                0 => "start",
                180 => "end",
                _ => "middle",
            };
            let _ = writeln!(out, r##"<text x="{}" y="{}" font-size="40" fill="#0a5a5a" text-anchor="{anchor}" dominant-baseline="central">{}</text>"##, inside.x, inside.y, esc(&pin.name));
        }
    }
}

fn sheet_svg(s: &Schematic, sheet: &SheetOut) -> String {
    let (w, h) = sheet.paper.size();
    let mut out = String::new();
    let _ = writeln!(out, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{}" height="{}" font-family="monospace">"#, w / 10, h / 10);
    let _ = writeln!(out, r##"<rect x="0" y="0" width="{w}" height="{h}" fill="#ffffff" stroke="#999" stroke-width="20"/>"##);
    let _ = writeln!(out, r##"<text x="300" y="300" font-size="80" fill="#555">{}</text>"##, esc(&sheet.name));
    for it in &sheet.items {
        match it {
            Item::Symbol { lib_id, at, orient, fields, .. } => {
                let Some(def) = s.symbols.iter().find(|d| &d.id == lib_id) else { continue };
                symbol(&mut out, def, *at, *orient);
                for f in fields.iter().filter(|f| f.visible) {
                    let p = *at + f.at;
                    let anchor = match f.justify {
                        Some(Justify::Left) => "start",
                        Some(Justify::Right) => "end",
                        _ => "middle",
                    };
                    let _ = writeln!(out, r##"<text x="{}" y="{}" font-size="50" fill="#333" text-anchor="{anchor}" dominant-baseline="central">{}</text>"##, p.x, p.y, esc(&f.value));
                }
            }
            Item::Wire { a, b } => {
                let _ = writeln!(out, r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#0a7a0a" stroke-width="10"/>"##, a.x, a.y, b.x, b.y);
            }
            Item::Junction(p) => {
                let _ = writeln!(out, r##"<circle cx="{}" cy="{}" r="20" fill="#0a7a0a"/>"##, p.x, p.y);
            }
            Item::NoConnect(p) => {
                let _ = writeln!(
                    out,
                    r##"<path d="M {} {} L {} {} M {} {} L {} {}" stroke="#0000c0" stroke-width="10"/>"##,
                    p.x - 30,
                    p.y - 30,
                    p.x + 30,
                    p.y + 30,
                    p.x - 30,
                    p.y + 30,
                    p.x + 30,
                    p.y - 30
                );
            }
            Item::Label { text, at, rot, global, .. } => {
                let d = dir_vec(*rot);
                let len = text_width(text, 50) + 40;
                let end = *at + d * len;
                let color = if *global { "#a05000" } else { "#0a5a5a" };
                if *global {
                    let _ = writeln!(out, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{color}" stroke-width="70" opacity="0.15"/>"#, at.x, at.y, end.x, end.y);
                }
                let mid = pt((at.x + end.x) / 2, (at.y + end.y) / 2);
                let (tx, ty, rotate) = if *rot == 90 || *rot == 270 { (mid.x - 25, mid.y, -90) } else { (mid.x, mid.y - 25, 0) };
                let _ = writeln!(
                    out,
                    r#"<text x="{tx}" y="{ty}" font-size="50" fill="{color}" text-anchor="middle" transform="rotate({rotate} {tx} {ty})">{}</text>"#,
                    esc(text)
                );
            }
            Item::Block { name, rect } => {
                let _ = writeln!(
                    out,
                    r##"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#6060c0" stroke-width="8" stroke-dasharray="40 25"/>"##,
                    rect.x0,
                    rect.y0,
                    rect.w(),
                    rect.h()
                );
                let _ = writeln!(out, r##"<text x="{}" y="{}" font-size="55" fill="#6060c0">{}</text>"##, rect.x0 + 20, rect.y0 - 25, esc(name));
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

/// One SVG per sheet.
pub fn render(s: &Schematic) -> Vec<(String, String)> {
    s.sheets.iter().map(|sh| (sh.name.clone(), sheet_svg(s, sh))).collect()
}
