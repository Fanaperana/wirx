//! `.esch` → `.wx` decompiler, so edits made in eleceda (by hand or by an AI through its MCP
//! server) can flow back into source.
//!
//! Connectivity is rebuilt the way eleceda's `netlist.ts` does it: union-find over points, points on
//! a wire (its interior too) join it, labels join by name and scope, power symbols by value. The
//! result is written as a wirx script: custom parts as `fn Name() { part(#{ … }) }`, one function
//! (or `group`) per block frame, `->` chains per net, rails as built-ins or `supply("…")`, and
//! `nc(…)` / `.driven()` for the flags.
//!
//! The output is checked the way `build` checks its drawing: it is compiled again and its netlist
//! compared with the one read from the file. That pass also decides which parts need an explicit
//! `ref:` (only those that automatic numbering would get wrong) and which repeated blocks can share
//! one function.

use crate::geom::{on_segment, Orient, Pt, Rect};
use crate::ir::PartDef;
use crate::sema::{self, natural_cmp};
use crate::stdlib;
use crate::symbol::boxgen::{auto_side, is_ground_name, BoxPin, Side};
use crate::symbol::templates::TEMPLATE_NAMES;
use crate::symbol::PinType;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

pub struct Imported {
    /// The `.wx` source.
    pub text: String,
    /// Things the `.wx` cannot say (dropped notes, buses, fields, …).
    pub warnings: Vec<String>,
    /// Differences between the file's connectivity and the compiled source; empty when faithful.
    pub mismatches: Vec<String>,
}

/// Decompile an eleceda project (`.esch` JSON). `design_name` names the design when recompiling.
pub fn import(esch: &Value, design_name: &str) -> Result<Imported, String> {
    if esch["format"] != "eleceda-schematic" {
        return Err("not an eleceda schematic (`format` is not \"eleceda-schematic\")".into());
    }
    let mut model = read(esch)?;
    let ctx = Ctx::new(&model);
    let mut plan = Plan::default();
    let file_name = format!("{design_name}.wx");
    for _ in 0..=model.comps.len() + 2 {
        let out = emit(&model, &ctx, &plan);
        let c = crate::check_source(&file_name, &out.text, Path::new("."));
        let Some(d) = c.design.filter(|d| d.instances.len() == out.order.len()) else {
            let msgs: Vec<String> = c.diags.iter().map(|x| x.message.clone()).collect();
            return Err(format!("the generated source does not compile (a wirx bug): {}", msgs.join("; ")));
        };
        // References: pin down only those that automatic numbering gets wrong.
        let mut changed = false;
        let mut lost = Vec::new();
        for (inst, &ci) in d.instances.iter().zip(&out.order) {
            let comp = &model.comps[ci];
            if !comp.ref_ok || inst.reference == comp.reference {
                continue;
            }
            if let Some(base) = out.shared.get(&ci) {
                changed |= plan.unshared.insert(base.clone());
            } else if ctx.parts[&comp.sym].kind == PartKind::Custom {
                lost.push(format!("{} becomes {}: custom parts take no `ref:` yet", comp.reference, inst.reference));
            } else {
                changed |= plan.explicit.insert(ci);
            }
        }
        if changed {
            continue;
        }
        let mismatches = compare(&model, &d, &out.order);
        model.warnings.extend(out.warnings);
        model.warnings.extend(lost);
        return Ok(Imported { text: out.text, warnings: model.warnings, mismatches });
    }
    Err("reference assignment did not settle (a wirx bug)".into())
}

// ---------------------------------------------------------------------------------------- model

struct SymPin {
    number: String,
    name: String,
    ptype: PinType,
    at: Pt,
    dir: u16,
    unit: u32,
    convert: u32,
    hidden: bool,
}

struct Sym {
    name: String,
    prefix: String,
    power: bool,
    pins: Vec<SymPin>,
    value: String,
    footprint: String,
    datasheet: String,
    description: String,
    open_pins_ok: bool,
    graphics: Value,
}

struct Comp {
    reference: String,
    /// The reference is annotated and unique, so it can be kept.
    ref_ok: bool,
    sym: String,
    value: String,
    footprint: String,
    /// Fields other than Reference, Value and Footprint, in file order.
    fields: Vec<(String, String)>,
    dnp: bool,
    sheet: usize,
    at: Pt,
    block: Option<usize>,
    /// Pin number → net; only pins that are drawn.
    pins: BTreeMap<String, usize>,
    nc: BTreeSet<String>,
}

#[derive(Default)]
struct NetInfo {
    pins: Vec<(usize, String)>,
    /// (priority, name) candidates: power 4, global label 3, local 2, hierarchical 1.
    names: Vec<(u8, String)>,
    /// Sheet of the first PWR_FLAG on the net.
    flag: Option<usize>,
}

impl NetInfo {
    fn best(&self) -> Option<&(u8, String)> {
        self.names.iter().max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
    }
}

struct Block {
    sheet: usize,
    name: String,
    rect: Rect,
}

struct Model {
    title: Vec<(String, String)>,
    sheets: Vec<String>,
    syms: HashMap<String, Sym>,
    comps: Vec<Comp>,
    nets: Vec<NetInfo>,
    blocks: Vec<Block>,
    warnings: Vec<String>,
    /// A decoupling capacitor wired straight to one chip's pin (a bank): cap → (chip, pin number).
    /// Written as `near(cap, chip.pin)`, so the bank is drawn at that pin again.
    near: HashMap<usize, (usize, String)>,
}

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

fn num(v: &Value) -> i32 {
    v.as_f64().unwrap_or(0.0).round() as i32
}
fn point(v: &Value) -> Pt {
    Pt { x: num(&v["x"]), y: num(&v["y"]) }
}
fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}
fn field(fields: &Value, name: &str) -> String {
    fields.as_array().into_iter().flatten().find(|f| f["name"] == name).map(|f| text(&f["value"])).unwrap_or_default()
}

fn read_sym(v: &Value) -> Sym {
    let pins = v["pins"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| SymPin {
            number: text(&p["number"]),
            name: text(&p["name"]),
            ptype: PinType::parse(p["type"].as_str().unwrap_or("")).unwrap_or(PinType::Passive),
            at: point(&p["at"]),
            dir: crate::geom::norm_rot(num(&p["dir"])),
            unit: p["unit"].as_u64().unwrap_or(0) as u32,
            convert: p["convert"].as_u64().unwrap_or(0) as u32,
            hidden: p["hidden"] == true,
        })
        .collect();
    Sym {
        name: text(&v["name"]),
        prefix: text(&v["refPrefix"]),
        power: v["power"] == true,
        pins,
        value: field(&v["fields"], "Value"),
        footprint: field(&v["fields"], "Footprint"),
        datasheet: field(&v["fields"], "Datasheet"),
        description: text(&v["description"]),
        open_pins_ok: v["openPinsOk"] == true,
        graphics: v["graphics"].clone(),
    }
}

fn read(esch: &Value) -> Result<Model, String> {
    let mut warnings = Vec::new();
    let mut syms = HashMap::new();
    for (id, v) in esch["symbols"].as_object().into_iter().flatten() {
        syms.insert(id.clone(), read_sym(v));
    }
    let sheets_v = esch["sheets"].as_array().ok_or("the file has no `sheets`")?;
    let mut title = Vec::new();
    for k in ["title", "revision", "company", "author", "date", "comment"] {
        if let Some(v) = esch["titleBlock"][k].as_str().filter(|v| !v.is_empty()) {
            title.push((k.to_string(), v.to_string()));
        }
    }

    let mut uf = Uf::default();
    // Connectivity through wires alone (no labels, no power symbols): what is drawn joined.
    let mut wired = Uf::default();
    let mut comps: Vec<Comp> = Vec::new();
    let mut comp_by_key: HashMap<String, usize> = HashMap::new();
    let mut pin_keys: Vec<BTreeMap<String, Vec<String>>> = Vec::new();
    let mut names_at: Vec<(String, u8, String)> = Vec::new();
    let mut flags: Vec<(String, usize)> = Vec::new();
    let mut ncs: HashSet<String> = HashSet::new();
    let mut blocks = Vec::new();
    let mut sheets = Vec::new();
    let (mut buses, mut notes, mut missing) = (0, 0, BTreeSet::new());

    for (si, sheet) in sheets_v.iter().enumerate() {
        sheets.push(text(&sheet["name"]));
        let sheet_id = text(&sheet["id"]);
        let key = |p: Pt| format!("{si}|{},{}", p.x, p.y);
        let items: &[Value] = sheet["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        let wires: Vec<(Pt, Pt)> = items.iter().filter(|i| i["kind"] == "wire").map(|i| (point(&i["a"]), point(&i["b"]))).collect();
        let mut pois: Vec<Pt> = Vec::new();
        for it in items {
            match it["kind"].as_str().unwrap_or("") {
                "wire" => {
                    let (a, b) = (point(&it["a"]), point(&it["b"]));
                    uf.union(&key(a), &key(b));
                    wired.union(&key(a), &key(b));
                    pois.extend([a, b]);
                }
                "junction" => pois.push(point(&it["at"])),
                "noconnect" => {
                    ncs.insert(key(point(&it["at"])));
                }
                "label" => {
                    let at = point(&it["at"]);
                    let t = text(&it["text"]);
                    let (scope, prio) = match it["labelType"].as_str().unwrap_or("local") {
                        "global" => (format!("G:{t}"), 3),
                        "hier" => (format!("H:{sheet_id}:{t}"), 1),
                        _ => (format!("L:{si}:{t}"), 2),
                    };
                    uf.union(&key(at), &scope);
                    names_at.push((key(at), prio, t));
                    pois.push(at);
                }
                "sheet" => {
                    let at = point(&it["at"]);
                    let (w, h) = (num(&it["size"]["w"]), num(&it["size"]["h"]));
                    for pin in it["pins"].as_array().into_iter().flatten() {
                        let off = num(&pin["offset"]);
                        let p = match pin["side"].as_str().unwrap_or("left") {
                            "right" => Pt { x: at.x + w, y: at.y + off },
                            "top" => Pt { x: at.x + off, y: at.y },
                            "bottom" => Pt { x: at.x + off, y: at.y + h },
                            _ => Pt { x: at.x, y: at.y + off },
                        };
                        uf.union(&key(p), &format!("H:{}:{}", text(&it["sheetId"]), text(&pin["name"])));
                        pois.push(p);
                    }
                }
                "symbol" => {
                    let lib_id = text(&it["libId"]);
                    let Some(def) = syms.get(&lib_id) else {
                        missing.insert(lib_id);
                        continue;
                    };
                    let at = point(&it["at"]);
                    let o = Orient { rot: crate::geom::norm_rot(num(&it["rot"])), mirror: it["mirror"] == true };
                    let unit = it["unit"].as_u64().unwrap_or(1) as u32;
                    let convert = it["convert"].as_u64().unwrap_or(1) as u32;
                    let placed: Vec<(&SymPin, String)> = def
                        .pins
                        .iter()
                        .filter(|p| (p.unit == 0 || p.unit == unit) && (p.convert == 0 || p.convert == convert))
                        .map(|p| (p, key(o.apply(p.at) + at)))
                        .collect();
                    for (p, k) in &placed {
                        pois.push(o.apply(p.at) + at);
                        uf.find(k);
                    }
                    if def.power {
                        let value = Some(field(&it["fields"], "Value")).filter(|v| !v.is_empty()).unwrap_or_else(|| def.name.clone());
                        for (p, k) in &placed {
                            if p.ptype == PinType::PowerIn {
                                uf.union(k, &format!("G:{value}"));
                                names_at.push((k.clone(), 4, value.clone()));
                            } else {
                                flags.push((k.clone(), si));
                            }
                        }
                        continue;
                    }
                    let reference = field(&it["fields"], "Reference");
                    let annotated = !reference.is_empty() && !reference.ends_with('?');
                    let ckey = if annotated { format!("{reference}\u{1}{lib_id}") } else { text(&it["id"]) };
                    let ci = *comp_by_key.entry(ckey).or_insert_with(|| {
                        let fields: Vec<(String, String)> = it["fields"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|f| (text(&f["name"]), text(&f["value"])))
                            .filter(|(k, _)| !matches!(k.as_str(), "Reference" | "Value" | "Footprint"))
                            .collect();
                        comps.push(Comp {
                            reference: reference.clone(),
                            ref_ok: annotated,
                            sym: lib_id.clone(),
                            value: field(&it["fields"], "Value"),
                            footprint: field(&it["fields"], "Footprint"),
                            fields,
                            dnp: it["dnp"] == true,
                            sheet: si,
                            at,
                            block: None,
                            pins: BTreeMap::new(),
                            nc: BTreeSet::new(),
                        });
                        pin_keys.push(BTreeMap::new());
                        comps.len() - 1
                    });
                    for (p, k) in placed {
                        pin_keys[ci].entry(p.number.clone()).or_default().push(k);
                    }
                }
                "block" => {
                    let at = point(&it["at"]);
                    let rect = Rect::new(at.x, at.y, at.x + num(&it["size"]["w"]), at.y + num(&it["size"]["h"]));
                    let name = text(&it["name"]);
                    // The frame wirx draws around a sheet's loose parts carries the sheet's name;
                    // those parts stay on the sheet rather than becoming a group.
                    if name != sheets[si] {
                        blocks.push(Block { sheet: si, name, rect });
                    }
                }
                "bus" | "busentry" => buses += 1,
                "text" => notes += 1,
                _ => {}
            }
        }
        for p in &pois {
            for (a, b) in &wires {
                if on_segment(*p, *a, *b) {
                    uf.union(&key(*p), &key(*a));
                    wired.union(&key(*p), &key(*a));
                }
            }
        }
    }
    if buses > 0 {
        warnings.push(format!("{buses} bus item(s) ignored: wirx has no buses yet, so nets joined only through a bus come apart"));
    }
    if notes > 0 {
        warnings.push(format!("{notes} text note(s) dropped"));
    }
    for m in missing {
        warnings.push(format!("symbol `{m}` is not embedded in the file; its instances are skipped"));
    }

    // A pin drawn twice (a shared pin of a multi-unit part) is one pin.
    for keys in &pin_keys {
        for ks in keys.values() {
            for k in &ks[1..] {
                uf.union(&ks[0], k);
            }
        }
    }
    let mut nets: Vec<NetInfo> = Vec::new();
    let mut net_of_root: HashMap<String, usize> = HashMap::new();
    for (ci, keys) in pin_keys.iter().enumerate() {
        let sym = &syms[&comps[ci].sym];
        for p in &sym.pins {
            let Some(ks) = keys.get(&p.number) else { continue };
            if comps[ci].pins.contains_key(&p.number) {
                continue;
            }
            let root = uf.find(&ks[0]);
            let n = *net_of_root.entry(root).or_insert_with(|| {
                nets.push(NetInfo::default());
                nets.len() - 1
            });
            nets[n].pins.push((ci, p.number.clone()));
            comps[ci].pins.insert(p.number.clone(), n);
            if ks.iter().any(|k| ncs.contains(k)) {
                comps[ci].nc.insert(p.number.clone());
            }
        }
    }
    for (k, prio, name) in names_at {
        if let Some(&n) = net_of_root.get(&uf.find(&k))
            && !nets[n].names.iter().any(|x| x.0 == prio && x.1 == name)
        {
            nets[n].names.push((prio, name));
        }
    }
    for (k, si) in flags {
        if let Some(&n) = net_of_root.get(&uf.find(&k)) {
            nets[n].flag.get_or_insert(si);
        }
    }
    for net in &nets {
        let power: Vec<&str> = net.names.iter().filter(|x| x.0 == 4).map(|x| x.1.as_str()).collect();
        if power.len() > 1 {
            warnings.push(format!("power nets {} are joined; the net keeps the name `{}`", power.join(", "), net.best().unwrap().1));
        }
    }

    // References that appear on more than one part are renumbered.
    let mut count: HashMap<String, usize> = HashMap::new();
    for c in &comps {
        *count.entry(c.reference.clone()).or_default() += 1;
    }
    for c in &mut comps {
        if c.ref_ok && count[&c.reference] > 1 {
            warnings.push(format!("reference {} is used more than once; wirx renumbers the copies", c.reference));
            c.ref_ok = false;
        }
    }
    // Each part belongs to the smallest block frame around it.
    for c in &mut comps {
        c.block = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.sheet == c.sheet && b.rect.contains(c.at))
            .min_by_key(|(_, b)| b.rect.w() as i64 * b.rect.h() as i64)
            .map(|(i, _)| i);
    }
    // Two-pin parts between power nets (decoupling capacitors) wired to the pins of one chip.
    let power = |n: usize| nets[n].names.iter().any(|x| x.0 == 4);
    let mut near = HashMap::new();
    for ci in 0..comps.len() {
        if comps[ci].pins.len() != 2 || !comps[ci].pins.values().all(|&n| power(n)) {
            continue;
        }
        let roots: Vec<String> = pin_keys[ci].values().map(|ks| wired.find(&ks[0])).collect();
        let mut at: Vec<(usize, String)> = Vec::new();
        for cj in 0..comps.len() {
            if cj == ci || comps[cj].pins.len() <= 2 {
                continue;
            }
            let sym = &syms[&comps[cj].sym];
            for (number, ks) in &pin_keys[cj] {
                let hidden = sym.pins.iter().any(|p| &p.number == number && p.hidden);
                if !hidden && roots.contains(&wired.find(&ks[0])) {
                    at.push((cj, number.clone()));
                }
            }
        }
        if let Some(first) = at.first()
            && at.iter().all(|x| x.0 == first.0)
        {
            let pin = at.iter().min_by(|a, b| natural_cmp(&a.1, &b.1)).unwrap().clone();
            near.insert(ci, pin);
        }
    }
    Ok(Model { title, sheets, syms, comps, nets, blocks, warnings, near })
}

// -------------------------------------------------------------------------------- part mapping

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartKind {
    /// A standard part: `R("10k", "0603", #{ … })`.
    Std,
    /// `Conn(n)` / `Conn2(rows)`.
    Conn,
    /// A part defined in the script: `fn Name() { part(#{ … }) }`, called as `Name()`.
    Custom,
}

struct PartInfo {
    kind: PartKind,
    /// The constructor: `R`, `Conn(6`, `ATmega328P`.
    call: String,
    /// `fn … { part(#{ … }) }` for custom parts.
    decl: Option<String>,
    pins: Vec<(String, String)>,
    value: String,
    footprint: String,
    datasheet: String,
    symbol: String,
    /// Fields given in the definition (so instances need not repeat them).
    fields: Vec<(String, String)>,
}

struct Ctx {
    parts: HashMap<String, PartInfo>,
    /// Function names already taken (custom parts).
    fn_names: HashSet<String>,
}

/// Names a script function or variable must not take: Rhai keywords and wirx built-ins.
const RESERVED: &[&str] = &[
    "let", "const", "fn", "if", "else", "for", "in", "while", "loop", "do", "until", "return", "throw", "try", "catch", "break", "continue",
    "switch", "import", "export", "as", "private", "true", "false", "this", "global", "Fn", "call", "curry", "type_of", "print", "debug",
    "eval", "is_def_var", "is_def_fn", "is_shared", "shared", "sync", "spawn", "thread", "go", "var", "static", "begin", "end", "match",
    "case", "public", "protected", "super", "new", "use", "module", "package", "with", "async", "await", "yield", "default", "void", "null",
    "nil", "net", "part", "sheet", "group", "nc", "near", "flow", "supply", "ground", "negative", "title", "revision", "author", "company",
    "date", "comment", "e24", "e96", "pin", "pins", "driven", "NC", "Conn", "Conn2", "left", "right", "top", "bottom", "up", "down",
    "hidden", "input", "output", "bidirectional", "tri_state", "passive", "free", "unspecified", "power_in", "power_out",
    "open_collector", "open_emitter", "no_connect",
];

fn reserved(s: &str) -> bool {
    RESERVED.contains(&s) || stdlib::find(s).is_some() || stdlib::RAILS.iter().any(|r| r.0 == s)
}

/// True for a plain Rhai identifier that is free to use as a name.
fn is_ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !reserved(s)
}

impl Ctx {
    fn new(m: &Model) -> Ctx {
        let mut parts: HashMap<String, PartInfo> = HashMap::new();
        let mut fn_names: HashSet<String> = HashSet::new();
        let used: BTreeSet<&String> = m.comps.iter().map(|c| &c.sym).collect();
        for id in used {
            let sym = &m.syms[id];
            let comps: Vec<&Comp> = m.comps.iter().filter(|c| &c.sym == id).collect();
            let mut pins: Vec<(String, String)> = Vec::new();
            for p in &sym.pins {
                if !pins.iter().any(|x| x.0 == p.number) {
                    pins.push((p.number.clone(), p.name.clone()));
                }
            }
            // wirx's own symbols go back to the standard part when the pins still match.
            if let Some(name) = id.strip_prefix("wirx:") {
                let std = stdlib::find(name).map(|p| (p.to_def(), PartKind::Std, name.to_string())).or_else(|| {
                    let def = sema::generated_part(name)?;
                    let n = def.pins.len();
                    let call = if def.symbol == "conn2" { format!("Conn2({}", n / 2) } else { format!("Conn({n}") };
                    Some((def, PartKind::Conn, call))
                });
                if let Some((def, kind, call)) = std
                    && def.pins.len() == pins.len()
                    && def.pins.iter().zip(&sym.pins).all(|(a, b)| a.number == b.number && a.name == b.name && a.ptype == b.ptype)
                {
                    parts.insert(
                        id.clone(),
                        PartInfo {
                            kind,
                            call,
                            decl: None,
                            pins,
                            value: def.value.clone(),
                            footprint: def.footprint.clone(),
                            datasheet: def.datasheet.clone(),
                            symbol: def.symbol.clone(),
                            fields: def.fields.clone(),
                        },
                    );
                    continue;
                }
            }
            // A custom part; its name is the part name and the function that makes it.
            let base = type_ident(id.strip_prefix("wirx:").unwrap_or(if sym.name.is_empty() { id } else { &sym.name }));
            let mut name = base.clone();
            let mut n = 2;
            while fn_names.contains(&name) || reserved(&name) {
                name = format!("{base}_{n}");
                n += 1;
            }
            fn_names.insert(name.clone());
            // Fields that every instance shares go into the definition.
            let mut fields: Vec<(String, String)> = Vec::new();
            if let Some(first) = comps.first() {
                for (k, v) in &first.fields {
                    if !v.is_empty() && comps.iter().all(|c| c.fields.iter().any(|f| &f.0 == k && &f.1 == v)) {
                        fields.push((k.clone(), v.clone()));
                    }
                }
            }
            let datasheet = fields.iter().find(|f| f.0 == "Datasheet").map(|f| f.1.clone()).unwrap_or_else(|| sym.datasheet.clone());
            let shape = detect_template(sym, &name);
            let decl = part_decl(&name, sym, &shape, &datasheet, &fields);
            let value = if sym.value.is_empty() { name.clone() } else { sym.value.clone() };
            parts.insert(
                id.clone(),
                PartInfo { kind: PartKind::Custom, call: name, decl: Some(decl), pins, value, footprint: sym.footprint.clone(), datasheet, symbol: shape, fields },
            );
        }
        Ctx { parts, fn_names }
    }
}

fn template_def(name: &str, sym: &Sym, shape: &str) -> PartDef {
    PartDef {
        name: name.into(),
        prefix: sym.prefix.clone(),
        footprint: String::new(),
        value: String::new(),
        description: String::new(),
        datasheet: String::new(),
        mpn: String::new(),
        fields: vec![],
        symbol: shape.into(),
        pins: sym
            .pins
            .iter()
            .map(|p| crate::ir::PartPin { number: p.number.clone(), name: p.name.clone(), ptype: p.ptype, side: None, hidden: p.hidden })
            .collect(),
        open_pins_ok: false,
        span: None,
        custom: None,
    }
}

/// The symbol shape wirx would draw exactly this way, else `box`.
fn detect_template(sym: &Sym, name: &str) -> String {
    for &shape in TEMPLATE_NAMES {
        if shape == "box" {
            continue;
        }
        let built = sema::build_symbol(&template_def(name, sym, shape));
        if built.pins.len() != sym.pins.len() {
            continue;
        }
        let same_pins = built.pins.iter().zip(&sym.pins).all(|(a, b)| a.at == b.at && a.dir == b.dir);
        if same_pins && serde_json::to_value(&built.graphics).ok().as_ref() == Some(&sym.graphics) {
            return shape.to_string();
        }
    }
    "box".into()
}

fn side_of(dir: u16) -> Side {
    match dir {
        0 => Side::Left,
        180 => Side::Right,
        90 => Side::Bottom,
        _ => Side::Top,
    }
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Left => "left",
        Side::Right => "right",
        Side::Top => "top",
        Side::Bottom => "bottom",
    }
}

/// A pin number in a pin table: an integer where it is one, else a string.
fn number_lit(n: &str) -> String {
    if n.parse::<i64>().is_ok_and(|v| v.to_string() == n) { n.to_string() } else { quote(n) }
}

fn part_decl(name: &str, sym: &Sym, shape: &str, datasheet: &str, fields: &[(String, String)]) -> String {
    let mut s = format!("fn {name}() {{\n    part(#{{\n");
    s += &format!("        name: {},\n", quote(name));
    s += &format!("        prefix: {},\n", quote(&sym.prefix));
    if !sym.value.is_empty() {
        s += &format!("        value: {},\n", quote(&sym.value));
    }
    if !sym.footprint.is_empty() {
        s += &format!("        footprint: {},\n", quote(&sym.footprint));
    }
    if !datasheet.is_empty() {
        s += &format!("        datasheet: {},\n", quote(datasheet));
    }
    if !sym.description.is_empty() {
        s += &format!("        description: {},\n", quote(&sym.description));
    }
    if let Some((_, v)) = fields.iter().find(|f| f.0 == "MPN") {
        s += &format!("        mpn: {},\n", quote(v));
    }
    let extra: Vec<String> = fields.iter().filter(|f| f.0 != "MPN" && f.0 != "Datasheet").map(|(k, v)| format!("{}: {}", map_key(k), quote(v))).collect();
    if !extra.is_empty() {
        s += &format!("        fields: #{{ {} }},\n", extra.join(", "));
    }
    if shape != "box" {
        s += &format!("        symbol: {},\n", quote(shape));
    }
    if sym.open_pins_ok {
        s += "        open_pins_ok: true,\n";
    }
    s += "        pins: [\n";
    let mut done: HashSet<&str> = HashSet::new();
    for p in &sym.pins {
        if !done.insert(&p.number) {
            continue;
        }
        let side = side_of(p.dir);
        // Pins with the same name, type and side share a row (`[[4, 6], "VCC", power_in]`); templates
        // map pins to slots in order, so they keep one row per pin.
        let same: Vec<&SymPin> = if shape == "box" && !matches!(p.name.as_str(), "" | "~") {
            sym.pins.iter().filter(|q| q.name == p.name && q.ptype == p.ptype && side_of(q.dir) == side && q.hidden == p.hidden).collect()
        } else {
            vec![p]
        };
        for q in &same {
            done.insert(&q.number);
        }
        let numbers: Vec<String> = same.iter().map(|q| number_lit(&q.number)).collect();
        let mut row = vec![if numbers.len() == 1 { numbers[0].clone() } else { format!("[{}]", numbers.join(", ")) }];
        row.push(quote(if p.name.is_empty() { "~" } else { &p.name }));
        if p.ptype != PinType::Passive {
            row.push(p.ptype.name().into());
        }
        if shape == "box" {
            let bp = BoxPin { number: p.number.clone(), name: p.name.clone(), ptype: p.ptype, side: None, hidden: p.hidden };
            if auto_side(&bp) != Some(side) {
                row.push(side_name(side).into());
            }
        }
        if p.hidden {
            row.push("hidden".into());
        }
        s += &format!("            [{}],\n", row.join(", "));
    }
    s += "        ],\n    })\n}\n";
    s
}

// ------------------------------------------------------------------------------------- tokens

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\t' => out += "\\t",
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A map key: bare when it is an identifier, else quoted.
fn map_key(k: &str) -> String {
    if k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        k.to_string()
    } else {
        quote(k)
    }
}

/// A function name for a custom part: letters, digits and `_`, starting with a letter.
fn type_ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if !out.starts_with(|c: char| c.is_ascii_alphabetic()) {
        out.insert(0, 'X');
    }
    out
}

/// A variable, parameter or group-function name: lower case.
fn local_ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    out = out.trim_matches('_').to_string();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    if !out.starts_with(|c: char| c.is_ascii_alphabetic()) {
        out.insert(0, 'x');
    }
    if reserved(&out) {
        out.push('_');
    }
    out
}

fn unique(base: String, taken: &mut HashSet<String>) -> String {
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

// -------------------------------------------------------------------------------------- emit

#[derive(Default)]
struct Plan {
    /// Group functions whose blocks are written as separate functions (so each can pin its references).
    unshared: HashSet<String>,
    /// Parts that get an explicit `ref:`.
    explicit: HashSet<usize>,
}

struct Emitted {
    text: String,
    /// Parts in the order the script creates them (the order of the compiled instances).
    order: Vec<usize>,
    /// Parts in a function written once for several blocks → that function's base name.
    shared: HashMap<usize, String>,
    warnings: Vec<String>,
}

/// One block on a sheet: its parts in canonical order.
struct BlockInst {
    block: usize,
    comps: Vec<usize>,
}

#[derive(Clone, PartialEq)]
enum ClassKind {
    /// Only inside one block: a chain of pins (named when the block is written once).
    Local,
    /// The same net in every block: written as that net.
    Fixed(usize),
    /// A parameter of the group function.
    Port(String),
}

/// A block written as a group function (called once per block), or as an inline `group(…)`.
struct GroupOut {
    /// Function name; `None` for an inline `group("name", || …)`.
    func: Option<String>,
    insts: Vec<BlockInst>,
    /// Net classes: the pins (position, number) of one net inside the block.
    classes: Vec<(BTreeSet<(usize, String)>, ClassKind)>,
    locals: Vec<String>,
}

fn part_key(ctx: &Ctx, c: &Comp) -> String {
    format!("{}\u{1}{}\u{1}{}\u{1}{:?}\u{1}{}", ctx.parts[&c.sym].call, c.value, c.footprint, c.fields, c.dnp)
}

fn classes_of(m: &Model, comps: &[usize]) -> BTreeMap<usize, BTreeSet<(usize, String)>> {
    let mut out: BTreeMap<usize, BTreeSet<(usize, String)>> = BTreeMap::new();
    for (pos, &ci) in comps.iter().enumerate() {
        for (number, &n) in &m.comps[ci].pins {
            out.entry(n).or_default().insert((pos, number.clone()));
        }
    }
    out
}

fn nc_of(m: &Model, comps: &[usize]) -> BTreeSet<(usize, String)> {
    comps.iter().enumerate().flat_map(|(pos, &ci)| m.comps[ci].nc.iter().map(move |n| (pos, n.clone()))).collect()
}

/// `b`'s parts in the order of their counterparts in `a` (matched by part, value and footprint,
/// then by reference order).
fn align(m: &Model, ctx: &Ctx, a: &[usize], b: &[usize]) -> Vec<usize> {
    let sorted = |v: &[usize]| {
        let mut v = v.to_vec();
        v.sort_by(|x, y| part_key(ctx, &m.comps[*x]).cmp(&part_key(ctx, &m.comps[*y])).then(natural_cmp(&m.comps[*x].reference, &m.comps[*y].reference)));
        v
    };
    let pair: HashMap<usize, usize> = sorted(a).into_iter().zip(sorted(b)).collect();
    a.iter().map(|c| pair[c]).collect()
}

fn compatible(m: &Model, ctx: &Ctx, a: &[usize], b: &[usize]) -> bool {
    let keys = |v: &[usize]| v.iter().map(|&c| part_key(ctx, &m.comps[c])).collect::<Vec<_>>();
    if a.len() != b.len() {
        return false;
    }
    let b = align(m, ctx, a, b);
    let b = b.as_slice();
    if keys(a) != keys(b) || nc_of(m, a) != nc_of(m, b) {
        return false;
    }
    let sets = |v: &[usize]| classes_of(m, v).into_values().collect::<BTreeSet<_>>();
    sets(a) == sets(b)
}

/// Pin expressions for the pins `numbers` of one part: `u.VCC` (covering both VCC pins) where the
/// name is unambiguous, else `u[3]` / `u["A9"]`.
fn pin_refs(info: &PartInfo, var: &str, numbers: &BTreeSet<String>, by_number: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut done: HashSet<&str> = HashSet::new();
    for (number, name) in &info.pins {
        if !numbers.contains(number) || done.contains(number.as_str()) {
            continue;
        }
        // Generic names like `Pin_3` read worse than the number.
        let generic = name.strip_prefix("Pin_") == Some(number.as_str());
        let usable = !by_number && !generic && !matches!(name.as_str(), "" | "~") && name != number && !info.pins.iter().any(|p| &p.0 == name);
        if usable {
            let same: Vec<&String> = info.pins.iter().filter(|p| &p.1 == name).map(|p| &p.0).collect();
            if same.iter().all(|n| numbers.contains(*n)) {
                let plain = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                out.push(if plain && !RESERVED.contains(&name.as_str()) { format!("{var}.{name}") } else { format!("{var}[{}]", quote(name)) });
                done.extend(same.iter().map(|s| s.as_str()));
                continue;
            }
        }
        out.push(format!("{var}[{}]", number_lit(number)));
        done.insert(number);
    }
    out
}

fn list(items: &[String]) -> String {
    if items.len() == 1 { items[0].clone() } else { format!("[{}]", items.join(", ")) }
}

/// `[a[1], b[2]] -> NET;`, or `a[1] -> [b[2], c[3]];` without a net; `None` when there is nothing
/// to join.
fn chain(net: Option<&str>, parts: &[(&PartInfo, &str, BTreeSet<String>)]) -> Option<String> {
    let total: usize = parts.iter().map(|p| p.2.len()).sum();
    let mut eps: Vec<String> = parts.iter().flat_map(|(info, var, nums)| pin_refs(info, var, nums, false)).collect();
    match net {
        Some(n) if !eps.is_empty() => Some(format!("{} -> {n};", list(&eps))),
        Some(_) => None,
        None if total < 2 => None,
        None => {
            if eps.len() < 2 {
                eps = parts.iter().flat_map(|(info, var, nums)| pin_refs(info, var, nums, true)).collect();
            }
            let first = eps.remove(0);
            Some(format!("{first} -> {};", list(&eps)))
        }
    }
}

/// The constructor call for one part: `R("5k1", "0603", #{ ref: "R1" })`.
fn construct(ctx: &Ctx, c: &Comp, explicit_ref: bool, warnings: &mut Vec<String>) -> String {
    let info = &ctx.parts[&c.sym];
    let mut fp_arg = None;
    if c.footprint != info.footprint {
        if c.footprint.contains(':') {
            let alias = ["0201", "0402", "0603", "0805", "1206", "1210", "2010", "2512"]
                .into_iter()
                .find(|s| sema::footprint_alias(&info.symbol, s).as_deref() == Some(c.footprint.as_str()));
            fp_arg = Some(quote(alias.unwrap_or(&c.footprint)));
        } else {
            warnings.push(format!("{}: footprint `{}` is not a `library:name` footprint; the part's default is kept", c.reference, c.footprint));
        }
    }
    let mut opts: Vec<String> = Vec::new();
    let value_differs = c.value != info.value;
    if value_differs && info.kind == PartKind::Conn {
        opts.push(format!("value: {}", quote(&c.value)));
    }
    if explicit_ref {
        opts.push(format!("ref: {}", quote(&c.reference)));
    }
    if c.dnp {
        opts.push("dnp: true".into());
    }
    for (k, v) in &c.fields {
        if v.is_empty() || info.fields.iter().any(|f| &f.0 == k && &f.1 == v) || (k == "Datasheet" && *v == info.datasheet) {
            continue;
        }
        if matches!(k.as_str(), "value" | "footprint" | "fp" | "ref" | "dnp" | "mpn") {
            warnings.push(format!("{}: field `{k}` clashes with a part option and is dropped", c.reference));
        } else {
            opts.push(format!("{}: {}", map_key(k), quote(v)));
        }
    }
    if info.kind == PartKind::Custom {
        if value_differs || fp_arg.is_some() || !opts.is_empty() {
            warnings.push(format!("{}: custom parts take no per-part value, footprint or fields yet; the part's own are kept", c.reference));
        }
        return format!("{}()", info.call);
    }
    let mut args: Vec<String> = Vec::new();
    if info.kind == PartKind::Std && (value_differs || fp_arg.is_some()) {
        args.push(quote(&c.value));
    }
    args.extend(fp_arg);
    if !opts.is_empty() {
        args.push(format!("#{{ {} }}", opts.join(", ")));
    }
    match info.kind {
        PartKind::Conn if args.is_empty() => format!("{})", info.call),
        PartKind::Conn => format!("{}, {})", info.call, args.join(", ")),
        _ => format!("{}({})", info.call, args.join(", ")),
    }
}

/// How a net is written: a built-in rail (`P5V`), `supply("VIO")`, or `net("SDA")`.
fn net_expr(name: &str, rail: bool) -> String {
    if !rail {
        return format!("net({})", quote(name));
    }
    if let Some(r) = stdlib::RAILS.iter().find(|r| r.1 == name) {
        return r.0.to_string();
    }
    let kind = if name.starts_with('-') {
        "negative"
    } else if is_ground_name(name) {
        "ground"
    } else {
        "supply"
    };
    format!("{kind}({})", quote(name))
}

fn emit(m: &Model, ctx: &Ctx, plan: &Plan) -> Emitted {
    let mut warnings = Vec::new();
    let by_ref = |a: &usize, b: &usize| natural_cmp(&m.comps[*a].reference, &m.comps[*b].reference).then(a.cmp(b));

    // Blocks with parts. A title like "ldo" or "ldo 2" (what wirx writes for a function) becomes a
    // function; other titles an inline `group("…", || …)`. A block of one part is always inline,
    // since a function that places one part is not a group.
    let mut fn_names: HashSet<String> = ctx.fn_names.clone();
    let mut by_base: Vec<(String, Vec<BlockInst>)> = Vec::new();
    let mut groups: Vec<GroupOut> = Vec::new();
    // Blocks in reference order (not drawing order), so the output does not depend on placement.
    let mut blocks: Vec<(usize, Vec<usize>)> = (0..m.blocks.len())
        .map(|bi| {
            let mut comps: Vec<usize> = (0..m.comps.len()).filter(|&c| m.comps[c].block == Some(bi)).collect();
            comps.sort_by(by_ref);
            (bi, comps)
        })
        .filter(|(_, comps)| !comps.is_empty())
        .collect();
    blocks.sort_by(|a, b| m.blocks[a.0].sheet.cmp(&m.blocks[b.0].sheet).then(by_ref(&a.1[0], &b.1[0])));
    for (bi, comps) in blocks {
        let b = &m.blocks[bi];
        let words: Vec<&str> = b.name.split_whitespace().collect();
        let base = match words.as_slice() {
            [f] | [f, _] if is_ident(f) && words.get(1).is_none_or(|n| n.parse::<u32>().is_ok()) && comps.len() > 1 => Some(f.to_string()),
            _ => None,
        };
        let inst = BlockInst { block: bi, comps };
        match base {
            Some(base) => match by_base.iter_mut().find(|x| x.0 == base) {
                Some(x) => x.1.push(inst),
                None => by_base.push((base, vec![inst])),
            },
            None => groups.push(GroupOut { func: None, insts: vec![inst], classes: vec![], locals: vec![] }),
        }
    }

    // Blocks of one function share it when they hold the same parts wired the same way.
    let mut shared: HashMap<usize, String> = HashMap::new();
    for (base, insts) in by_base {
        let mut clusters: Vec<Vec<BlockInst>> = Vec::new();
        for bi in insts {
            let fit = if plan.unshared.contains(&base) { None } else { clusters.iter_mut().find(|cl| compatible(m, ctx, &cl[0].comps, &bi.comps)) };
            match fit {
                Some(cl) => {
                    let comps = align(m, ctx, &cl[0].comps, &bi.comps);
                    cl.push(BlockInst { comps, ..bi })
                }
                None => clusters.push(vec![bi]),
            }
        }
        for cl in clusters {
            if cl.len() > 1 {
                for bi in &cl {
                    for &c in &bi.comps {
                        shared.insert(c, base.clone());
                    }
                }
            }
            let name = unique(base.clone(), &mut fn_names);
            groups.push(GroupOut { func: Some(name), insts: cl, classes: vec![], locals: vec![] });
        }
    }
    let block_group: HashMap<usize, (usize, usize)> =
        groups.iter().enumerate().flat_map(|(gi, g)| g.insts.iter().enumerate().map(move |(k, bi)| (bi.block, (gi, k)))).collect();

    // Which nets are rails: named by a power symbol, or by a label with a rail name.
    let is_rail_name = |s: &str| stdlib::RAILS.iter().any(|r| r.1 == s) || sema::auto_rail(s).is_some();
    let rail: Vec<bool> = m.nets.iter().map(|n| n.names.iter().any(|x| x.0 == 4) || n.best().is_some_and(|b| is_rail_name(&b.1))).collect();
    let scope_of = |c: usize| (m.comps[c].sheet, m.comps[c].block.filter(|b| block_group.contains_key(b)));
    let scopes: Vec<HashSet<(usize, Option<usize>)>> = m.nets.iter().map(|n| n.pins.iter().map(|p| scope_of(p.0)).collect()).collect();

    // Classify each group's nets.
    let mut bound: HashSet<usize> = HashSet::new();
    for g in &mut groups {
        let per_inst: Vec<BTreeMap<BTreeSet<(usize, String)>, usize>> =
            g.insts.iter().map(|bi| classes_of(m, &bi.comps).into_iter().map(|(n, s)| (s, n)).collect()).collect();
        let mut classes: Vec<(BTreeSet<(usize, String)>, ClassKind)> = Vec::new();
        for set in per_inst[0].keys() {
            let nets: Vec<usize> = per_inst.iter().map(|x| x[set]).collect();
            let internal = |n: usize| !rail[n] && m.nets[n].flag.is_none() && scopes[n].len() == 1;
            let kind = if nets.iter().all(|&n| internal(n)) {
                ClassKind::Local
            } else if nets.iter().all(|&n| n == nets[0]) {
                ClassKind::Fixed(nets[0])
            } else {
                bound.extend(nets.iter().copied());
                ClassKind::Port(String::new())
            };
            classes.push((set.clone(), kind));
        }
        classes.sort_by(|a, b| a.0.iter().next().cmp(&b.0.iter().next()));
        g.classes = classes;
    }

    // Net names: from labels and power symbols, else generated where a net must be named (it
    // crosses a group or sheet, is a parameter, or is driven).
    let mut net_name: Vec<Option<String>> = vec![None; m.nets.len()];
    let mut taken: HashSet<String> = HashSet::new();
    let mut named: Vec<usize> = (0..m.nets.len()).filter(|&n| m.nets[n].best().is_some()).collect();
    named.sort_by(|&a, &b| {
        let (x, y) = (m.nets[a].best().unwrap(), m.nets[b].best().unwrap());
        y.0.cmp(&x.0).then(natural_cmp(&x.1, &y.1))
    });
    for n in named {
        net_name[n] = Some(unique(m.nets[n].best().unwrap().1.clone(), &mut taken));
    }
    for n in 0..m.nets.len() {
        let needs = m.nets[n].flag.is_some() || bound.contains(&n) || scopes[n].len() > 1;
        if net_name[n].is_some() || !needs {
            continue;
        }
        // Like wirx's own names: a named pin of the part with the most pins (`U1_XTAL1`).
        let pins = &m.nets[n].pins;
        let size = |c: usize| ctx.parts[&m.comps[c].sym].pins.len();
        let pin_name = |p: &(usize, String)| ctx.parts[&m.comps[p.0].sym].pins.iter().find(|x| x.0 == p.1).map(|x| x.1.clone()).unwrap_or_default();
        let best = pins
            .iter()
            .filter(|p| !matches!(pin_name(p).as_str(), "" | "~") && pin_name(p) != p.1)
            .max_by(|a, b| size(a.0).cmp(&size(b.0)).then(by_ref(&b.0, &a.0)));
        let label = match best {
            Some(p) => format!("{}_{}", m.comps[p.0].reference, pin_name(p)),
            None => {
                let p = pins.iter().min_by(|a, b| by_ref(&a.0, &b.0).then(a.1.cmp(&b.1))).unwrap();
                format!("{}_{}", m.comps[p.0].reference, p.1)
            }
        };
        net_name[n] = Some(unique(label, &mut taken));
    }
    // A label that renames a rail apart (a second "GND") is still a rail, under its new name.
    let expr = |n: usize| net_name[n].as_ref().map(|name| net_expr(name, rail[n]));

    let mut out = String::new();
    out += "// Imported from eleceda by `wirx import`.\n";
    for (k, v) in &m.title {
        out += &format!("{k}({});\n", quote(v));
    }
    let mut decls: Vec<&PartInfo> = ctx.parts.values().filter(|p| p.decl.is_some()).collect();
    decls.sort_by(|a, b| a.call.cmp(&b.call));
    for p in decls {
        out += "\n";
        out += p.decl.as_ref().unwrap();
    }

    // Variables, parameters and bodies of the groups.
    for g in &mut groups {
        let first = &g.insts[0].comps;
        let shared_fn = g.insts.len() > 1;
        let mut names: HashSet<String> = HashSet::new();
        let mut count: HashMap<String, usize> = HashMap::new();
        let prefix = |c: usize| local_ident(&m.syms[&m.comps[c].sym].prefix);
        for &c in first {
            *count.entry(prefix(c)).or_default() += 1;
        }
        let mut idx: HashMap<String, usize> = HashMap::new();
        g.locals = first
            .iter()
            .map(|&c| {
                let p = prefix(c);
                let base = if count[&p] == 1 {
                    p
                } else {
                    let k = idx.entry(p.clone()).or_default();
                    *k += 1;
                    format!("{p}{k}")
                };
                unique(base, &mut names)
            })
            .collect();
        for (set, kind) in &mut g.classes {
            let ClassKind::Port(port) = kind else { continue };
            let (pos, number) = set.iter().next().unwrap();
            let label = g.insts.iter().find_map(|bi| {
                let n = m.comps[bi.comps[*pos]].pins[number];
                m.nets[n].best().filter(|_| !rail[n]).map(|b| local_ident(&b.1))
            });
            *port = unique(label.unwrap_or_else(|| format!("{}_{}", g.locals[*pos], local_ident(number))), &mut names);
        }
        if let Some(f) = &g.func {
            let ports: Vec<&str> = g.classes.iter().filter_map(|(_, k)| if let ClassKind::Port(p) = k { Some(p.as_str()) } else { None }).collect();
            out += &format!("\nfn {f}({}) {{\n", ports.join(", "));
            out += &body(m, ctx, plan, g, 0, shared_fn, &expr, "    ", &mut warnings);
            out += "}\n";
        }
    }

    // Sheets.
    let mut order: Vec<usize> = Vec::new();
    let sheet_names: Vec<String> = {
        let mut seen = HashSet::new();
        m.sheets.iter().map(|s| unique(if s.is_empty() { "Sheet".into() } else { s.clone() }, &mut seen)).collect()
    };
    for (si, sheet) in sheet_names.iter().enumerate() {
        out += &format!("\nsheet({}, || {{\n", quote(sheet));
        let mut loose: Vec<usize> =
            (0..m.comps.len()).filter(|&c| m.comps[c].sheet == si && !m.comps[c].block.is_some_and(|b| block_group.contains_key(&b))).collect();
        loose.sort_by(by_ref);
        let mut vars: HashSet<String> = HashSet::new();
        let mut var: HashMap<usize, String> = HashMap::new();
        for &c in &loose {
            let comp = &m.comps[c];
            let base = if comp.ref_ok { local_ident(&comp.reference) } else { format!("{}_new", local_ident(&m.syms[&comp.sym].prefix)) };
            let name = unique(base, &mut vars);
            let explicit = comp.ref_ok && plan.explicit.contains(&c);
            out += &format!("    let {name} = {};\n", construct(ctx, comp, explicit, &mut warnings));
            order.push(c);
            var.insert(c, name);
        }
        let mut placed: Vec<&BlockInst> = groups.iter().flat_map(|g| g.insts.iter()).filter(|bi| m.blocks[bi.block].sheet == si).collect();
        placed.sort_by(|a, b| by_ref(&a.comps.iter().copied().min_by(by_ref).unwrap(), &b.comps.iter().copied().min_by(by_ref).unwrap()));
        for bi in placed {
            let (gi, k) = block_group[&bi.block];
            let g = &groups[gi];
            match &g.func {
                Some(f) => {
                    let args: Vec<String> = g
                        .classes
                        .iter()
                        .filter_map(|(set, kind)| {
                            let ClassKind::Port(_) = kind else { return None };
                            let (pos, number) = set.iter().next().unwrap();
                            expr(m.comps[g.insts[k].comps[*pos]].pins[number])
                        })
                        .collect();
                    out += &format!("    {f}({});\n", args.join(", "));
                }
                None => {
                    out += &format!("    group({}, || {{\n", quote(&m.blocks[bi.block].name));
                    out += &body(m, ctx, plan, g, k, false, &expr, "        ", &mut warnings);
                    out += "    });\n";
                }
            }
            order.extend(&bi.comps);
        }
        // Nets among the parts placed directly on this sheet.
        let mut lines: Vec<(String, String)> = Vec::new();
        for n in 0..m.nets.len() {
            let mut here: Vec<&(usize, String)> = m.nets[n].pins.iter().filter(|p| var.contains_key(&p.0)).collect();
            if here.is_empty() {
                continue;
            }
            here.sort_by(|a, b| by_ref(&a.0, &b.0));
            let mut parts: Vec<(&PartInfo, &str, BTreeSet<String>)> = Vec::new();
            for (c, number) in here {
                match parts.last_mut() {
                    Some(p) if p.1 == var[c] => {
                        p.2.insert(number.clone());
                    }
                    _ => parts.push((&ctx.parts[&m.comps[*c].sym], &var[c], BTreeSet::from([number.clone()]))),
                }
            }
            if let Some(line) = chain(expr(n).as_deref(), &parts) {
                let key = net_name[n].clone().unwrap_or_else(|| format!("\u{ffff}{line}"));
                lines.push((key, line));
            }
        }
        lines.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        for (_, l) in lines {
            out += &format!("    {l}\n");
        }
        let nc: Vec<String> = loose
            .iter()
            .flat_map(|&c| {
                let var = &var;
                m.comps[c].nc.iter().flat_map(move |number| pin_refs(&ctx.parts[&m.comps[c].sym], &var[&c], &BTreeSet::from([number.clone()]), false))
            })
            .collect();
        if !nc.is_empty() {
            out += &format!("    nc({});\n", list(&nc));
        }
        for &c in &loose {
            if let Some((chip, number)) = m.near.get(&c)
                && let Some(cv) = var.get(chip)
                && let Some(pin) = pin_refs(&ctx.parts[&m.comps[*chip].sym], cv, &BTreeSet::from([number.clone()]), false).first()
            {
                out += &format!("    near({}, {pin});\n", var[&c]);
            }
        }
        let mut flags: Vec<usize> = (0..m.nets.len()).filter(|&n| m.nets[n].flag == Some(si)).collect();
        flags.sort_by(|a, b| natural_cmp(net_name[*a].as_deref().unwrap_or(""), net_name[*b].as_deref().unwrap_or("")));
        for n in flags {
            out += &format!("    {}.driven();\n", expr(n).unwrap_or_default());
        }
        out += "});\n";
    }
    Emitted { text: out, order, shared, warnings }
}

/// The statements of a group: its parts, then one chain per net, then `nc(…)`. `k` picks the block
/// whose parts are written (only explicit refs and inline groups depend on it).
#[allow(clippy::too_many_arguments)]
fn body(
    m: &Model,
    ctx: &Ctx,
    plan: &Plan,
    g: &GroupOut,
    k: usize,
    shared_fn: bool,
    expr: &dyn Fn(usize) -> Option<String>,
    indent: &str,
    warnings: &mut Vec<String>,
) -> String {
    let comps = &g.insts[k].comps;
    let mut s = String::new();
    for (pos, &c) in comps.iter().enumerate() {
        let explicit = !shared_fn && m.comps[c].ref_ok && plan.explicit.contains(&c);
        s += &format!("{indent}let {} = {};\n", g.locals[pos], construct(ctx, &m.comps[c], explicit, warnings));
    }
    for (set, kind) in &g.classes {
        let (p0, n0) = set.iter().next().unwrap();
        let net = m.comps[comps[*p0]].pins[n0];
        let head = match kind {
            ClassKind::Port(p) => Some(p.clone()),
            ClassKind::Fixed(n) => expr(*n),
            // A local net keeps its label when the block is written once; in a shared function the
            // name would join the copies.
            ClassKind::Local if !shared_fn => expr(net),
            ClassKind::Local => None,
        };
        let mut parts: Vec<(&PartInfo, &str, BTreeSet<String>)> = Vec::new();
        for (pos, number) in set {
            match parts.last_mut() {
                Some(p) if p.1 == g.locals[*pos] => {
                    p.2.insert(number.clone());
                }
                _ => parts.push((&ctx.parts[&m.comps[comps[*pos]].sym], &g.locals[*pos], BTreeSet::from([number.clone()]))),
            }
        }
        if let Some(line) = chain(head.as_deref(), &parts) {
            s += &format!("{indent}{line}\n");
        }
    }
    let nc: Vec<String> = nc_of(m, comps)
        .into_iter()
        .flat_map(|(pos, number)| pin_refs(&ctx.parts[&m.comps[comps[pos]].sym], &g.locals[pos], &BTreeSet::from([number]), false))
        .collect();
    if !nc.is_empty() {
        s += &format!("{indent}nc({});\n", list(&nc));
    }
    for (pos, &c) in comps.iter().enumerate() {
        if let Some((chip, number)) = m.near.get(&c)
            && let Some(cp) = comps.iter().position(|x| x == chip)
            && let Some(pin) = pin_refs(&ctx.parts[&m.comps[*chip].sym], &g.locals[cp], &BTreeSet::from([number.clone()]), false).first()
        {
            s += &format!("{indent}near({}, {pin});\n", g.locals[pos]);
        }
    }
    s
}

// ------------------------------------------------------------------------------------ compare

/// Nets of the file vs nets of the compiled source, as sets of (part, pin number). Compiled
/// instances map to parts by creation order.
fn compare(m: &Model, d: &crate::ir::Design, order: &[usize]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ours: BTreeSet<BTreeSet<(usize, String)>> = BTreeSet::new();
    for net in &d.nets {
        let set: BTreeSet<(usize, String)> = net.pins.iter().map(|&(i, k)| (order[i], d.pin(i, k).number.clone())).collect();
        if set.len() > 1 {
            ours.insert(set);
        }
    }
    let seen: HashSet<usize> = order.iter().copied().collect();
    for (c, comp) in m.comps.iter().enumerate() {
        if !seen.contains(&c) {
            problems.push(format!("{} is missing from the source", comp.reference));
        }
    }
    let theirs: BTreeSet<BTreeSet<(usize, String)>> =
        m.nets.iter().map(|n| n.pins.iter().cloned().collect::<BTreeSet<_>>()).filter(|s| s.len() > 1).collect();
    let show = |s: &BTreeSet<(usize, String)>| s.iter().map(|(c, p)| format!("{}.{p}", m.comps[*c].reference)).collect::<Vec<_>>().join(" ");
    for s in theirs.difference(&ours) {
        problems.push(format!("net {{{}}} is not in the source", show(s)));
    }
    for s in ours.difference(&theirs) {
        problems.push(format!("the source has net {{{}}}, which the file does not", show(s)));
    }
    problems
}
