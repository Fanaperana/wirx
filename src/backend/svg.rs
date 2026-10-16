//! SVG preview of the laid-out sheets, drawn the way eleceda draws them: the same drawing frame
//! (border, zone markers, title block), the light theme's colours and the sheet font. Good for a
//! quick look by people and AI without opening eleceda.

use crate::geom::{dir_vec, text_width, Orient, Pt};
use crate::ir::Design;
use crate::layout::{Item, Schematic, SheetOut};
use crate::symbol::{Fill, Graphic, Justify, SymbolDef};
use std::fmt::Write;

// eleceda's light canvas theme.
const PAGE: &str = "#f5f4ef";
const BORDER: &str = "#8a1c1c";
const WIRE: &str = "#008400";
const BODY: &str = "#840000";
const BODY_FILL: &str = "#ffffc2";
const PIN_NAME: &str = "#006464";
const PIN_NUMBER: &str = "#a4002c";
const FIELD: &str = "#006464";
const LABEL: &str = "#000000";
const GLOBAL_LABEL: &str = "#840000";
const NO_CONNECT: &str = "#0000a4";
const BLOCK: &str = "#000084";
const FONT: &str = "'Courier Prime', 'Courier New', monospace";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn tf(o: Orient, at: Pt, p: Pt) -> Pt {
    o.apply(p) + at
}

fn symbol(out: &mut String, def: &SymbolDef, at: Pt, o: Orient) {
    let fill = |f: &Option<Fill>| match f {
        Some(Fill::Background) => BODY_FILL,
        Some(Fill::Outline) => BODY,
        _ => "none",
    };
    for g in &def.graphics {
        match g {
            Graphic::Polyline { points, fill: f, width, .. } => {
                let pts: Vec<String> = points.iter().map(|p| tf(o, at, *p)).map(|p| format!("{},{}", p.x, p.y)).collect();
                let _ = writeln!(out, r#"<polyline points="{}" fill="{}" stroke="{BODY}" stroke-width="{}" stroke-linejoin="round"/>"#, pts.join(" "), fill(f), width.unwrap_or(8).max(6));
            }
            Graphic::Rect { start, end, fill: f, width, .. } => {
                let (a, b) = (tf(o, at, *start), tf(o, at, *end));
                let _ = writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="{BODY}" stroke-width="{}"/>"#,
                    a.x.min(b.x),
                    a.y.min(b.y),
                    (a.x - b.x).abs(),
                    (a.y - b.y).abs(),
                    fill(f),
                    width.unwrap_or(8).max(6)
                );
            }
            Graphic::Circle { center, radius, width, .. } => {
                let c = tf(o, at, *center);
                let _ = writeln!(out, r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{BODY}" stroke-width="{}"/>"#, c.x, c.y, radius, width.unwrap_or(8).max(6));
            }
            Graphic::Arc { start, mid, end, width, .. } => {
                let (a, m, b) = (tf(o, at, *start), tf(o, at, *mid), tf(o, at, *end));
                let _ = writeln!(
                    out,
                    r#"<path d="M {} {} Q {} {} {} {}" fill="none" stroke="{BODY}" stroke-width="{}"/>"#,
                    a.x,
                    a.y,
                    2 * m.x - (a.x + b.x) / 2,
                    2 * m.y - (a.y + b.y) / 2,
                    b.x,
                    b.y,
                    width.unwrap_or(8).max(6)
                );
            }
            Graphic::Text { at: p, text, size, .. } => {
                let c = tf(o, at, *p);
                let _ = writeln!(out, r#"<text x="{}" y="{}" font-size="{}" fill="{BODY}" text-anchor="middle" dominant-baseline="central">{}</text>"#, c.x, c.y, size.unwrap_or(50), esc(text));
            }
        }
    }
    for pin in &def.pins {
        if pin.hidden == Some(true) || pin.length == 0 {
            continue;
        }
        let dir = o.angle(pin.dir);
        let a = tf(o, at, pin.at);
        let b = a + dir_vec(dir) * pin.length;
        let _ = writeln!(out, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{BODY}" stroke-width="6"/>"#, a.x, a.y, b.x, b.y);
        let horizontal = dir == 0 || dir == 180;
        // Name inside the body, number above the pin line.
        if def.hide_pin_names != Some(true) && pin.name != "~" && !pin.name.is_empty() {
            let inside = b + dir_vec(dir) * 25;
            let (anchor, rotate) = match dir {
                0 => ("start", 0),
                180 => ("end", 0),
                90 => ("start", -90),
                _ => ("end", -90),
            };
            let _ = writeln!(
                out,
                r#"<text x="{}" y="{}" font-size="45" fill="{PIN_NAME}" text-anchor="{anchor}" dominant-baseline="central" transform="rotate({rotate} {} {})">{}</text>"#,
                inside.x,
                inside.y,
                inside.x,
                inside.y,
                esc(&pin.name)
            );
        }
        if def.hide_pin_numbers != Some(true) {
            let mid = Pt { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
            let (x, y, rotate) = if horizontal { (mid.x, mid.y - 15, 0) } else { (mid.x - 15, mid.y, -90) };
            let _ = writeln!(out, r#"<text x="{x}" y="{y}" font-size="40" fill="{PIN_NUMBER}" text-anchor="middle" transform="rotate({rotate} {x} {y})">{}</text>"#, esc(&pin.number));
        }
    }
}

/// Today's date (UTC) as YYYY-MM-DD, for title blocks with no date (as eleceda shows them).
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Border, zone markers and title block: the same drawing frame eleceda puts on every sheet.
fn frame(out: &mut String, d: &Design, sheet: &SheetOut, index: usize, count: usize) {
    let (w, h) = sheet.paper.size();
    const OUT: i32 = 300;
    const ZONE: i32 = 100;
    let inner = (OUT + ZONE, OUT + ZONE, w - 2 * (OUT + ZONE), h - 2 * (OUT + ZONE));
    let cols = (((w - 2 * OUT) as f64 / 1970.0).round() as i32).max(4);
    let rows = (((h - 2 * OUT) as f64 / 1970.0).round() as i32).max(3);
    let col_w = (w - 2 * OUT) as f64 / cols as f64;
    let row_h = (h - 2 * OUT) as f64 / rows as f64;
    let _ = writeln!(out, r#"<g font-family="{FONT}" fill="{BORDER}">"#);
    let _ = writeln!(out, r#"<rect x="{OUT}" y="{OUT}" width="{}" height="{}" fill="none" stroke="{BORDER}" stroke-width="10"/>"#, w - 2 * OUT, h - 2 * OUT);
    let _ = writeln!(out, r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{BORDER}" stroke-width="6"/>"#, inner.0, inner.1, inner.2, inner.3);
    for i in 0..cols {
        let x = OUT as f64 + i as f64 * col_w;
        if i > 0 {
            let _ = writeln!(out, r#"<line x1="{x}" y1="{OUT}" x2="{x}" y2="{}" stroke="{BORDER}" stroke-width="5"/>"#, OUT + ZONE);
            let _ = writeln!(out, r#"<line x1="{x}" y1="{}" x2="{x}" y2="{}" stroke="{BORDER}" stroke-width="5"/>"#, h - OUT - ZONE, h - OUT);
        }
        let cx = x + col_w / 2.0;
        for y in [OUT + ZONE / 2, h - OUT - ZONE / 2] {
            let _ = writeln!(out, r#"<text x="{cx}" y="{y}" font-size="62" text-anchor="middle" dominant-baseline="central">{}</text>"#, i + 1);
        }
    }
    for i in 0..rows {
        let y = OUT as f64 + i as f64 * row_h;
        if i > 0 {
            let _ = writeln!(out, r#"<line x1="{OUT}" y1="{y}" x2="{}" y2="{y}" stroke="{BORDER}" stroke-width="5"/>"#, OUT + ZONE);
            let _ = writeln!(out, r#"<line x1="{}" y1="{y}" x2="{}" y2="{y}" stroke="{BORDER}" stroke-width="5"/>"#, w - OUT - ZONE, w - OUT);
        }
        let cy = y + row_h / 2.0;
        let letter = (b'A' + i as u8) as char;
        for x in [OUT + ZONE / 2, w - OUT - ZONE / 2] {
            let _ = writeln!(out, r#"<text x="{x}" y="{cy}" font-size="62" text-anchor="middle" dominant-baseline="central">{letter}</text>"#);
        }
    }

    // Title block, bottom right inside the inner border.
    const TB_W: i32 = 4600;
    const TB_H: i32 = 1350;
    let (tx, ty) = (inner.0 + inner.2 - TB_W, inner.1 + inner.3 - TB_H);
    let meta = |k: &str| d.meta.get(k).cloned().unwrap_or_default();
    let title = Some(meta("title")).filter(|t| !t.is_empty()).unwrap_or_else(|| d.name.clone());
    let date = Some(meta("date")).filter(|t| !t.is_empty()).unwrap_or_else(today);
    let rev = Some(meta("revision")).filter(|t| !t.is_empty()).unwrap_or_else(|| "—".into());
    let company = meta("company");
    let author = meta("author");
    let (who_label, who) = if !company.is_empty() { ("Company", company) } else { ("Comment", meta("comment")) };
    let who = if author.is_empty() { who } else if who.is_empty() { author } else { format!("{who} · {author}") };
    let _ = writeln!(out, r#"<g transform="translate({tx} {ty})">"#);
    let _ = writeln!(out, r#"<rect width="{TB_W}" height="{TB_H}" fill="{PAGE}" stroke="{BORDER}" stroke-width="8"/>"#);
    for (x1, y1, x2, y2) in [(0, 300, TB_W, 300), (0, 720, TB_W, 720), (0, 1020, TB_W, 1020), (1500, 720, 1500, 1020), (3100, 720, 3100, 1350)] {
        let _ = writeln!(out, r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{BORDER}" stroke-width="5"/>"#);
    }
    let texts = [
        (60, 190, 80, "start", "normal", 0.75, format!("Sheet: /{}/", sheet.name)),
        (TB_W - 60, 190, 80, "end", "normal", 0.75, format!("File: {}.esch", d.name)),
        (60, 420, 70, "start", "normal", 0.7, "Title:".to_string()),
        (60, 640, 170, "start", "700", 1.0, title),
        (60, 820, 62, "start", "normal", 0.7, "Size".to_string()),
        (60, 955, 100, "start", "700", 1.0, sheet.paper.name().to_string()),
        (1560, 820, 62, "start", "normal", 0.7, "Date".to_string()),
        (1560, 955, 100, "start", "normal", 1.0, date),
        (3160, 820, 62, "start", "normal", 0.7, "Rev".to_string()),
        (3160, 955, 100, "start", "700", 1.0, rev),
        (60, 1130, 62, "start", "normal", 0.7, who_label.to_string()),
        (60, 1265, 90, "start", "normal", 1.0, who),
        (3160, 1130, 62, "start", "normal", 0.7, "Sheet".to_string()),
        (3160, 1265, 100, "start", "700", 1.0, format!("{} / {}", index + 1, count)),
        (TB_W - 60, 1265, 62, "end", "normal", 0.55, "wirx · eleceda".to_string()),
    ];
    for (x, y, size, anchor, weight, opacity, text) in texts {
        let _ = writeln!(out, r#"<text x="{x}" y="{y}" font-size="{size}" text-anchor="{anchor}" font-weight="{weight}" opacity="{opacity}">{}</text>"#, esc(&text));
    }
    out.push_str("</g>\n</g>\n");
}

fn sheet_svg(d: &Design, s: &Schematic, sheet: &SheetOut, index: usize) -> String {
    let (w, h) = sheet.paper.size();
    let mut out = String::new();
    let _ = writeln!(out, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{}" height="{}" font-family="{FONT}">"#, w / 10, h / 10);
    let _ = writeln!(out, r#"<rect x="0" y="0" width="{w}" height="{h}" fill="{PAGE}"/>"#);
    frame(&mut out, d, sheet, index, s.sheets.len());
    for it in &sheet.items {
        match it {
            Item::Block { name, rect } => {
                let _ = writeln!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{BLOCK}" stroke-width="6" stroke-dasharray="50 30" opacity="0.7"/>"#,
                    rect.x0,
                    rect.y0,
                    rect.w(),
                    rect.h()
                );
                let _ = writeln!(out, r#"<text x="{}" y="{}" font-size="58" fill="{BLOCK}">{}</text>"#, rect.x0 + 20, rect.y0 - 25, esc(name));
            }
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
                    let color = if def.is_power() { BODY } else { FIELD };
                    let _ = writeln!(out, r#"<text x="{}" y="{}" font-size="50" fill="{color}" text-anchor="{anchor}" dominant-baseline="central">{}</text>"#, p.x, p.y, esc(&f.value));
                }
            }
            Item::Wire { a, b } => {
                let _ = writeln!(out, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{WIRE}" stroke-width="8" stroke-linecap="round"/>"#, a.x, a.y, b.x, b.y);
            }
            Item::Junction(p) => {
                let _ = writeln!(out, r#"<circle cx="{}" cy="{}" r="22" fill="{WIRE}"/>"#, p.x, p.y);
            }
            Item::NoConnect(p) => {
                let _ = writeln!(
                    out,
                    r#"<path d="M {} {} L {} {} M {} {} L {} {}" stroke="{NO_CONNECT}" stroke-width="8"/>"#,
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
                let dv = dir_vec(*rot);
                let vertical = *rot == 90 || *rot == 270;
                if *global {
                    // Pennant pointing both ways, as eleceda draws a bidirectional global label.
                    let h = 37;
                    let len = text_width(text, 50) + 40 + 2 * h;
                    let n = Pt { x: -dv.y, y: dv.x };
                    let along = |t: i32, s: i32| *at + dv * t + n * s;
                    let pts = [along(0, 0), along(h, -h), along(len - h, -h), along(len, 0), along(len - h, h), along(h, h), along(0, 0)];
                    let pts: Vec<String> = pts.iter().map(|p| format!("{},{}", p.x, p.y)).collect();
                    let _ = writeln!(out, r#"<polyline points="{}" fill="none" stroke="{GLOBAL_LABEL}" stroke-width="6"/>"#, pts.join(" "));
                    let c = along(len / 2, 0);
                    let rotate = if vertical { -90 } else { 0 };
                    let _ = writeln!(
                        out,
                        r#"<text x="{}" y="{}" font-size="50" fill="{GLOBAL_LABEL}" text-anchor="middle" dominant-baseline="central" transform="rotate({rotate} {} {})">{}</text>"#,
                        c.x,
                        c.y,
                        c.x,
                        c.y,
                        esc(text)
                    );
                } else {
                    // Local label: text sitting on the wire, reading away from the anchor.
                    let anchor = if *rot == 0 || *rot == 90 { "start" } else { "end" };
                    let (x, y, rotate) = if vertical { (at.x - 10, at.y, -90) } else { (at.x, at.y - 10, 0) };
                    let _ = writeln!(out, r#"<text x="{x}" y="{y}" font-size="50" fill="{LABEL}" text-anchor="{anchor}" transform="rotate({rotate} {x} {y})">{}</text>"#, esc(text));
                }
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

/// One SVG per sheet.
pub fn render(d: &Design, s: &Schematic) -> Vec<(String, String)> {
    s.sheets.iter().enumerate().map(|(i, sh)| (sh.name.clone(), sheet_svg(d, s, sh, i))).collect()
}
