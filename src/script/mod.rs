//! The Rhai front end: a `.wx` file is a Rhai script whose functions build the design.
//!
//! Script values are small handles into a shared [`Builder`]: [`Part`] (an instance), [`PinRef`]
//! (one or more same-named pins), [`Net`] (a named net, rail or anonymous net) and [`NoConnect`].
//! `a -> b` joins its two sides and evaluates to `b`; a two-pin part in the middle of a chain
//! passes through (left side to its *in* pin, the chain continues from its *out* pin).
//!
//! Groups come from the call stack: every call of a script function that places parts is a
//! framed group named after the function (a debugger hook gives wirx the stack). Calls nested in
//! another function stay in the outer group; `group(name, || …)` names one explicitly.

mod values;

use crate::diag::{Diagnostic, Sources, Span};
use crate::ir::{Design, PartDef, PartPin};
use crate::sema::{auto_rail, footprint_alias, generated_part, suggest, Builder, NewInstance};
use crate::stdlib;
use crate::symbol::boxgen::{layout_symbol, Side, Slot};
use crate::symbol::templates::{self, TEMPLATE_NAMES};
use crate::symbol::{library, PinType, RailKind, SymbolDef};
use rhai::module_resolvers::FileModuleResolver;
use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext, Position, Scope};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

type RResult<T> = Result<T, Box<EvalAltResult>>;

/// A part instance.
#[derive(Clone, Debug)]
pub struct Part {
    pub id: usize,
}

/// One pin, or all pins sharing a name (`u.GND` on a part with three GND pins).
#[derive(Clone, Debug)]
pub struct PinRef {
    pub inst: usize,
    pub pins: Vec<usize>,
}

/// A named net, a rail or an anonymous net.
#[derive(Clone, Debug)]
pub struct Net {
    pub node: usize,
    pub name: String,
}

/// `NC`: the pins on the other side of `->` are intentionally unconnected.
#[derive(Clone, Debug)]
pub struct NoConnect;

struct State {
    b: Builder,
    /// Folder of the main file: library paths are relative to it.
    base_dir: PathBuf,
    /// Symbol libraries already read, by path.
    libraries: HashMap<PathBuf, Vec<SymbolDef>>,
    sources: Sources,
    source_ids: HashMap<String, usize>,
    main_source: String,
    design_name: String,
    sheet_stack: Vec<usize>,
    group_stack: Vec<usize>,
    root_groups: HashMap<usize, usize>,
    /// (sheet, function, source, call position) -> group
    fn_groups: HashMap<(usize, String, String, Position), usize>,
    fn_group_names: HashMap<(usize, String), usize>,
    /// Groups created for a function call (may be dissolved when they hold one part).
    fn_group_set: HashSet<usize>,
    /// Names of native functions (everything that is not a user script function).
    natives: HashSet<String>,
    /// A wirx error raised while the script ran; reported instead of Rhai's generic message.
    pending: Option<Diagnostic>,
}

type Shared = Rc<RefCell<State>>;

impl State {
    fn file_id(&mut self, source: Option<&str>) -> Option<usize> {
        let name = source.unwrap_or(&self.main_source).to_string();
        if let Some(&id) = self.source_ids.get(&name) {
            return Some(id);
        }
        let text = std::fs::read_to_string(&name).ok()?;
        let id = self.sources.add(name.clone(), text);
        self.source_ids.insert(name, id);
        Some(id)
    }

    fn span(&mut self, source: Option<&str>, pos: Position) -> Option<Span> {
        let (line, col) = (pos.line()?, pos.position().unwrap_or(1));
        let file = self.file_id(source)?;
        let text = &self.sources.files[file].text;
        let mut offset = 0;
        for (i, l) in text.split_inclusive('\n').enumerate() {
            if i + 1 == line {
                let mut col_off: usize = l.chars().take(col.saturating_sub(1)).map(char::len_utf8).sum();
                // `u.VIN`: point at the name, not the dot.
                if l[col_off..].starts_with('.') {
                    col_off += 1;
                }
                let start = offset + col_off;
                let end = start + l[col_off..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum::<usize>().max(1);
                return Some(Span::new(file, start, end.min(text.len())));
            }
            offset += l.len();
        }
        None
    }

    fn current_sheet(&mut self) -> usize {
        if let Some(&s) = self.sheet_stack.last() {
            return s;
        }
        let name = self.design_name.clone();
        self.b.add_sheet(&name)
    }

    fn root_group(&mut self, sheet: usize) -> usize {
        if let Some(&g) = self.root_groups.get(&sheet) {
            return g;
        }
        let name = self.b.design.sheets[sheet].name.clone();
        let g = self.b.add_group(name, sheet, false);
        self.root_groups.insert(sheet, g);
        g
    }
}

fn ctx_span(st: &Shared, ctx: &NativeCallContext) -> Option<Span> {
    st.borrow_mut().span(ctx.call_source(), ctx.call_position())
}

/// Raise a wirx error at the current call: it stops the script and is reported with its hint.
fn raise<T>(st: &Shared, ctx: &NativeCallContext, d: Diagnostic) -> RResult<T> {
    let span = ctx_span(st, ctx);
    let msg = d.message.clone();
    st.borrow_mut().pending = Some(d.at_opt(span));
    Err(Box::new(EvalAltResult::ErrorRuntime(msg.into(), ctx.call_position())))
}

/// The group parts created by the current call belong to (see the module docs).
fn current_group(st: &Shared, ctx: &NativeCallContext) -> usize {
    let frames: Vec<(String, String, Position)> = ctx
        .global_runtime_state()
        .debugger()
        .call_stack()
        .iter()
        .map(|f| (f.fn_name.to_string(), f.source.as_ref().map(|s| s.to_string()).unwrap_or_default(), f.pos))
        .collect();
    let mut s = st.borrow_mut();
    let sheet = s.current_sheet();
    let last_ctx = frames.iter().rposition(|f| f.0 == "sheet" || f.0 == "group");
    if let Some(i) = last_ctx
        && frames[i].0 == "group"
        && let Some(&g) = s.group_stack.last()
    {
        return g;
    }
    let start = last_ctx.map(|i| i + 1).unwrap_or(0);
    let user = frames[start..].iter().find(|f| !f.0.starts_with("anon$") && !s.natives.contains(&f.0));
    match user {
        None => s.root_group(sheet),
        Some((name, source, pos)) => {
            let key = (sheet, name.clone(), source.clone(), *pos);
            if let Some(&g) = s.fn_groups.get(&key) {
                return g;
            }
            let count = s.fn_group_names.entry((sheet, name.clone())).or_insert(0);
            *count += 1;
            let label = if *count == 1 { name.clone() } else { format!("{name} {count}") };
            let g = s.b.add_group(label, sheet, true);
            s.fn_groups.insert(key, g);
            s.fn_group_set.insert(g);
            g
        }
    }
}

fn type_name(v: &Dynamic) -> String {
    if v.is::<Part>() {
        "a part".into()
    } else if v.is::<PinRef>() {
        "a pin".into()
    } else if v.is::<Net>() {
        "a net".into()
    } else if v.is_string() {
        "a string".into()
    } else {
        format!("a {}", v.type_name())
    }
}

/// In and out pins of a two-pin part used inline: anode before cathode, + before -, else 1 → 2.
fn through_pins(part: &PartDef) -> Option<(usize, usize)> {
    if part.pins.len() != 2 {
        return None;
    }
    let idx = |n: &str| part.pins.iter().position(|p| p.name == n);
    if let (Some(a), Some(k)) = (idx("A"), idx("K")) {
        return Some((a, k));
    }
    if let (Some(p), Some(m)) = (idx("+"), idx("-")) {
        return Some((p, m));
    }
    Some((0, 1))
}

/// Nodes a value stands for on one side of `->` (`right` picks a part's in pin, else its out pin).
fn nodes_of(st: &Shared, ctx: &NativeCallContext, v: &Dynamic, right: bool, out: &mut Vec<usize>, pins: &mut Vec<(usize, usize)>) -> RResult<()> {
    if let Some(p) = v.clone().try_cast::<PinRef>() {
        let s = st.borrow();
        for &k in &p.pins {
            out.push(s.b.pin_node(p.inst, k));
            pins.push((p.inst, k));
        }
        return Ok(());
    }
    if let Some(n) = v.clone().try_cast::<Net>() {
        out.push(n.node);
        return Ok(());
    }
    if let Some(p) = v.clone().try_cast::<Part>() {
        let (part, npins) = {
            let s = st.borrow();
            let part = s.b.design.parts[s.b.design.instances[p.id].part].clone();
            let n = part.pins.len();
            (part, n)
        };
        let Some((i, o)) = through_pins(&part) else {
            let names: Vec<String> = part.pins.iter().take(6).map(|p| p.name.clone()).collect();
            return raise(
                st,
                ctx,
                Diagnostic::error("not_inline", format!("`{}` has {npins} pins, so it cannot sit inline in a chain", part.name))
                    .hint(format!("connect one of its pins instead, e.g. `u.{}` (pins: {}…)", names[0], names.join(", "))),
            );
        };
        let k = if right { i } else { o };
        out.push(st.borrow().b.pin_node(p.id, k));
        pins.push((p.id, k));
        return Ok(());
    }
    if v.is_array() {
        for item in v.clone().into_array().unwrap() {
            nodes_of(st, ctx, &item, right, out, pins)?;
        }
        return Ok(());
    }
    if v.is_string() {
        let s = v.clone().into_string().unwrap();
        return raise(
            st,
            ctx,
            Diagnostic::error("string_as_net", format!("\"{s}\" is a string, not a net")).hint(format!("use `net(\"{s}\")` for a named net, or `supply(\"{s}\")` for a rail")),
        );
    }
    raise(st, ctx, Diagnostic::error("bad_endpoint", format!("cannot connect {}", type_name(v))).hint("connect pins (`u.VIN`, `c[1]`), nets, rails, parts or arrays of them"))
}

fn link(st: &Shared, ctx: &NativeCallContext, a: Dynamic, b: Dynamic) -> RResult<Dynamic> {
    if a.is::<NoConnect>() {
        return raise(st, ctx, Diagnostic::error("bad_nc", "`NC` goes on the right: `u.PB0 -> NC`"));
    }
    let mut left = Vec::new();
    let mut left_pins = Vec::new();
    nodes_of(st, ctx, &a, false, &mut left, &mut left_pins)?;
    if b.is::<NoConnect>() {
        if left_pins.is_empty() {
            return raise(st, ctx, Diagnostic::error("bad_nc", "only pins can be marked `NC`"));
        }
        let mut s = st.borrow_mut();
        for (i, k) in left_pins {
            s.b.set_nc(i, k);
        }
        return Ok(b);
    }
    let mut right = Vec::new();
    nodes_of(st, ctx, &b, true, &mut right, &mut Vec::new())?;
    let mut s = st.borrow_mut();
    // `u.VCC -> C(..)`: remember which pin the part was wired to.
    let pin_part = |p: &Dynamic, q: &Dynamic| -> Option<(usize, usize, usize)> {
        let pin = p.clone().try_cast::<PinRef>()?;
        let part = q.clone().try_cast::<Part>()?;
        (!pin.pins.is_empty() && pin.inst != part.id).then(|| (part.id, pin.inst, pin.pins[0]))
    };
    if let Some(h) = pin_part(&a, &b).or_else(|| pin_part(&b, &a)) {
        let g = s.b.design.instances[h.0].group;
        s.b.design.groups[g].chained.push(h);
    }
    let all: Vec<usize> = left.into_iter().chain(right).collect();
    for w in all.windows(2) {
        s.b.union(w[0], w[1]);
    }
    Ok(b)
}

fn pin_lookup(st: &Shared, ctx: &NativeCallContext, p: &Part, key: &str) -> RResult<PinRef> {
    let part = {
        let s = st.borrow();
        s.b.design.parts[s.b.design.instances[p.id].part].clone()
    };
    let mut idx: Vec<usize> = part.pins.iter().enumerate().filter(|(_, x)| x.number == key).map(|(k, _)| k).collect();
    if idx.is_empty() {
        idx = part.pins.iter().enumerate().filter(|(_, x)| x.name == key).map(|(k, _)| k).collect();
    }
    if idx.is_empty() {
        // Library names: `GPIO40/ADC0` answers to GPIO40 and ADC0, `~{QSPI_SS}` to QSPI_SS.
        let plain = |n: &str| n.trim_start_matches("~{").trim_start_matches('~').trim_end_matches('}').to_string();
        idx = part.pins.iter().enumerate().filter(|(_, x)| x.name.split('/').any(|seg| plain(seg) == key)).map(|(k, _)| k).collect();
    }
    if idx.is_empty() {
        let names: Vec<String> = part.pins.iter().flat_map(|x| [x.number.clone(), x.name.clone()]).collect();
        let list: Vec<String> =
            part.pins.iter().map(|x| if x.name == "~" || x.name == x.number { x.number.clone() } else { format!("{} {}", x.number, x.name) }).collect();
        let hint = match suggest(key, &names) {
            Some(s) if s.chars().all(|c| c.is_ascii_digit()) => format!("did you mean `[{s}]`? pins: {}", list.join(", ")),
            Some(s) => format!("did you mean `.{s}`? pins: {}", list.join(", ")),
            None => format!("pins: {}", list.join(", ")),
        };
        return raise(st, ctx, Diagnostic::error("unknown_pin", format!("part `{}` has no pin `{key}`", part.name)).hint(hint));
    }
    Ok(PinRef { inst: p.id, pins: idx })
}

/// `R("10k")`, `R("10k", "0603")`, `R("10k", "0603", #{ ref: "R5" })` for a known part definition.
fn instantiate(st: &Shared, ctx: &NativeCallContext, def: PartDef, args: &[Dynamic]) -> RResult<Part> {
    let symbol = def.symbol.clone();
    let mut value = def.value.clone();
    let mut footprint = def.footprint.clone();
    let mut fields = def.fields.clone();
    if !def.mpn.is_empty() {
        fields.push(("MPN".into(), def.mpn.clone()));
    }
    if !def.datasheet.is_empty() {
        fields.push(("Datasheet".into(), def.datasheet.clone()));
    }
    let mut dnp = false;
    let mut reference = None;
    let set_fp = |st: &Shared, ctx: &NativeCallContext, v: &Dynamic, footprint: &mut String| -> RResult<()> {
        let text = v.to_string();
        if text.contains(':') {
            *footprint = text;
        } else if let Some(f) = footprint_alias(&symbol, &text) {
            *footprint = f;
        } else {
            return raise(
                st,
                ctx,
                Diagnostic::error("unknown_footprint", format!("\"{text}\" is not a footprint alias for `{}`", def.name))
                    .hint("give a full footprint name like \"Package_SO:SOIC-8_3.9x4.9mm_P1.27mm\""),
            );
        }
        Ok(())
    };
    for (i, a) in args.iter().enumerate() {
        if a.is_unit() {
            continue;
        }
        match i {
            0 if a.is_map() => {}
            0 => value = values::value_text(a, &symbol).ok_or(()).or_else(|_| raise(st, ctx, Diagnostic::error("bad_value", format!("a value must be a string or number, not {}", type_name(a)))))?,
            1 if !a.is_map() => set_fp(st, ctx, a, &mut footprint)?,
            _ => {}
        }
        if let Some(m) = a.read_lock::<Map>() {
            for (k, v) in m.iter() {
                match k.as_str() {
                    "value" => value = values::value_text(v, &symbol).unwrap_or_else(|| v.to_string()),
                    "footprint" | "fp" => set_fp(st, ctx, v, &mut footprint)?,
                    "ref" => reference = Some(v.to_string()),
                    "dnp" => dnp = v.as_bool().unwrap_or(false),
                    "mpn" => {
                        fields.retain(|f| f.0 != "MPN");
                        fields.push(("MPN".into(), v.to_string()));
                    }
                    other => {
                        fields.retain(|f| f.0 != other);
                        fields.push((other.to_string(), v.to_string()));
                    }
                }
            }
        }
    }
    let span = ctx_span(st, ctx);
    let group = current_group(st, ctx);
    let mut s = st.borrow_mut();
    let part = match s.b.add_part(def) {
        Ok(p) => p,
        Err(e) => {
            drop(s);
            return raise(st, ctx, Diagnostic::error("duplicate_part", e));
        }
    };
    let sheet = s.b.design.groups[group].sheet;
    let id = s.b.add_instance(NewInstance { part, value, footprint, fields, dnp, reference, group, sheet, span });
    Ok(Part { id })
}

/// `part(#{ name: …, pins: [ … ] })`: a custom part definition, instantiated.
fn custom_part(st: &Shared, ctx: &NativeCallContext, m: Map) -> RResult<Part> {
    let get = |k: &str| m.get(k).map(|v| v.to_string());
    let Some(name) = get("name") else {
        return raise(st, ctx, Diagnostic::error("part_without_name", "`part` needs a `name`").hint("part(#{ name: \"MyChip\", pins: [ … ] })"));
    };
    let mut def = PartDef {
        name: name.clone(),
        prefix: get("prefix").unwrap_or_else(|| "U".into()),
        footprint: get("footprint").unwrap_or_default(),
        value: get("value").unwrap_or_else(|| name.clone()),
        description: get("description").unwrap_or_default(),
        datasheet: get("datasheet").unwrap_or_default(),
        mpn: get("mpn").unwrap_or_default(),
        fields: vec![],
        symbol: get("symbol").unwrap_or_else(|| "box".into()),
        pins: vec![],
        open_pins_ok: m.get("open_pins_ok").and_then(|v| v.as_bool().ok()).unwrap_or(false),
        span: ctx_span(st, ctx),
        custom: None,
    };
    const KEYS: &[&str] = &["name", "prefix", "footprint", "value", "description", "datasheet", "mpn", "symbol", "library", "open_pins_ok", "fields", "pins"];
    for k in m.keys() {
        if !KEYS.contains(&k.as_str()) {
            return raise(st, ctx, Diagnostic::error("unknown_property", format!("unknown part property `{k}`")).hint(format!("properties: {}", KEYS.join(", "))));
        }
    }
    if let Some(f) = m.get("fields").and_then(|v| v.read_lock::<Map>().map(|m| m.clone())) {
        def.fields = f.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    }
    // A symbol from a library file, or one laid out here: the pins come from the symbol.
    let library_sym = match m.get("library") {
        Some(lib) => Some(library_symbol(st, ctx, &lib.to_string(), &get("symbol").unwrap_or_else(|| name.clone()))?),
        None => match m.get("symbol").and_then(|v| v.read_lock::<Map>().map(|m| m.clone())) {
            Some(layout) => Some(laid_out_symbol(st, ctx, &name, &def.prefix, &layout)?),
            None => None,
        },
    };
    if let Some(sym) = library_sym {
        if m.contains_key("pins") {
            return raise(
                st,
                ctx,
                Diagnostic::error("pins_twice", format!("part `{name}` takes its pins from its symbol; drop `pins`")).hint("list the pins inside `symbol: #{ left: [...], ... }`, or use the library symbol's pins"),
            );
        }
        if sym.unit_count > 1 {
            return raise(st, ctx, Diagnostic::error("multi_unit_symbol", format!("symbol `{}` has {} units; wirx draws single-unit symbols only for now", sym.name, sym.unit_count)));
        }
        let field = |k: &str| sym.fields.iter().find(|f| f.name == k).map(|f| f.value.clone()).filter(|v| !v.is_empty());
        if def.footprint.is_empty() {
            def.footprint = field("Footprint").unwrap_or_default();
        }
        if def.datasheet.is_empty() {
            def.datasheet = field("Datasheet").filter(|d| d != "~").unwrap_or_default();
        }
        if def.description.is_empty() {
            def.description = sym.description.clone().unwrap_or_default();
        }
        if get("prefix").is_none() && !sym.ref_prefix.is_empty() {
            def.prefix = sym.ref_prefix.trim_end_matches('?').to_string();
        }
        def.symbol = "custom".into();
        def.pins = sym.pins.iter().map(|p| PartPin { number: p.number.clone(), name: p.name.clone(), ptype: p.ptype, side: None, hidden: p.hidden == Some(true) }).collect();
        let mut numbers = HashSet::new();
        for p in &def.pins {
            if !numbers.insert(p.number.clone()) {
                return raise(st, ctx, Diagnostic::error("duplicate_pin", format!("pin number `{}` appears twice in symbol `{}`", p.number, sym.name)));
            }
        }
        def.custom = Some(Box::new(sym));
        return instantiate(st, ctx, def, &[]);
    }
    if def.symbol != "box" && def.symbol != "conn" && templates::template(&def.symbol).is_none() {
        let names: Vec<String> = TEMPLATE_NAMES.iter().map(|s| s.to_string()).collect();
        let hint = suggest(&def.symbol, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("shapes: {}", TEMPLATE_NAMES.join(", ")));
        return raise(st, ctx, Diagnostic::error("unknown_symbol", format!("unknown symbol shape `{}`", def.symbol)).hint(hint));
    }
    let Some(pins) = m.get("pins").and_then(|v| v.clone().into_array().ok()) else {
        return raise(st, ctx, Diagnostic::error("part_without_pins", format!("part `{name}` has no `pins`")).hint("pins: [[1, \"A\"], [2, \"B\"]]"));
    };
    let mut seen = HashSet::new();
    for entry in pins {
        let Ok(row) = entry.clone().into_array() else {
            return raise(st, ctx, Diagnostic::error("bad_pin", format!("pin entries are arrays like [1, \"VIN\", power_in], not {}", type_name(&entry))));
        };
        if row.len() < 2 {
            return raise(st, ctx, Diagnostic::error("bad_pin", "a pin needs at least a number and a name: [1, \"VIN\"]"));
        }
        let numbers: Vec<String> = match row[0].clone().into_array() {
            Ok(list) => list.iter().map(|v| v.to_string()).collect(),
            Err(_) => vec![row[0].to_string()],
        };
        let pname = row[1].to_string();
        let mut ptype = PinType::Passive;
        let mut side = None;
        let mut hidden = false;
        for extra in &row[2..] {
            let t = extra.to_string();
            if let Some(pt) = PinType::parse(&t) {
                ptype = pt;
            } else if let Some(sd) = Side::parse(&t) {
                side = Some(sd);
            } else if t == "hidden" {
                hidden = true;
            } else {
                return raise(
                    st,
                    ctx,
                    Diagnostic::error("bad_pin", format!("`{t}` is not a pin type or side (pin {pname})"))
                        .hint("types: input, output, bidirectional, tri_state, passive, power_in, power_out, open_collector, open_emitter, free, unspecified, no_connect; sides: left, right, top, bottom"),
                );
            }
        }
        for n in numbers {
            if !seen.insert(n.clone()) {
                return raise(st, ctx, Diagnostic::error("duplicate_pin", format!("pin number `{n}` is declared twice in part `{name}`")));
            }
            def.pins.push(PartPin { number: n, name: pname.clone(), ptype, side, hidden });
        }
    }
    if let Some(t) = templates::template(&def.symbol)
        && t.slots.len() != def.pins.len()
    {
        return raise(
            st,
            ctx,
            Diagnostic::error(
                "template_pin_count",
                format!("symbol `{}` has {} pins ({}), but part `{name}` declares {}", def.symbol, t.slots.len(), t.slot_names, def.pins.len()),
            )
            .hint(format!("declare the pins in the order {}", t.slot_names)),
        );
    }
    instantiate(st, ctx, def, &[])
}

/// `library: "libs/rp.elib", symbol: "RP2350B"`: a symbol from an eleceda or KiCad library file.
fn library_symbol(st: &Shared, ctx: &NativeCallContext, path: &str, name: &str) -> RResult<SymbolDef> {
    let full = st.borrow().base_dir.join(path);
    if !st.borrow().libraries.contains_key(&full) {
        match library::load(&full) {
            Ok(list) => {
                st.borrow_mut().libraries.insert(full.clone(), list);
            }
            Err(e) => return raise(st, ctx, Diagnostic::error("library_not_found", e).hint("paths are relative to the design file; libraries are .elib, .esch or .kicad_sym")),
        }
    }
    let s = st.borrow();
    let list = &s.libraries[&full];
    match library::find(list, name) {
        Some(sym) => Ok(sym.clone()),
        None => {
            let names: Vec<String> = list.iter().map(|x| x.name.clone()).collect();
            let hint = suggest(name, &names).map(|g| format!("did you mean `{g}`?")).unwrap_or_else(|| format!("{} symbols in this library", names.len()));
            drop(s);
            raise(st, ctx, Diagnostic::error("unknown_symbol", format!("`{path}` has no symbol `{name}`")).hint(hint))
        }
    }
}

/// `symbol: #{ left: [...], right: [...], top: [...], bottom: [...] }`: a box symbol with pins
/// where the author puts them. Each side lists pin rows `[number(s), name, type]` from top to
/// bottom (left to right on top and bottom), and `gap` for a blank place between groups. A row
/// with several numbers is drawn once, the other numbers stacked behind it.
fn laid_out_symbol(st: &Shared, ctx: &NativeCallContext, name: &str, prefix: &str, layout: &Map) -> RResult<SymbolDef> {
    const KEYS: &[&str] = &["left", "right", "top", "bottom", "width", "height"];
    for k in layout.keys() {
        if !KEYS.contains(&k.as_str()) {
            return raise(st, ctx, Diagnostic::error("unknown_property", format!("unknown symbol layout key `{k}`")).hint(format!("keys: {}", KEYS.join(", "))));
        }
    }
    let mut sides: [Vec<Slot>; 4] = [vec![], vec![], vec![], vec![]];
    for (i, side) in ["left", "right", "top", "bottom"].iter().enumerate() {
        let Some(rows) = layout.get(*side) else { continue };
        let Ok(rows) = rows.clone().into_array() else {
            return raise(st, ctx, Diagnostic::error("bad_symbol", format!("`{side}` must be a list of pin rows")));
        };
        for row in rows {
            if row.is_string() && row.to_string() == "gap" {
                sides[i].push(Slot::Gap);
                continue;
            }
            let Ok(r) = row.clone().into_array() else {
                return raise(st, ctx, Diagnostic::error("bad_pin", format!("symbol rows are `gap` or [number(s), name, type], not {}", type_name(&row))));
            };
            if r.len() < 2 {
                return raise(st, ctx, Diagnostic::error("bad_pin", "a pin row needs a number and a name: [35, \"RUN\", input]"));
            }
            let numbers: Vec<String> = match r[0].clone().into_array() {
                Ok(list) => list.iter().map(|v| v.to_string()).collect(),
                Err(_) => vec![r[0].to_string()],
            };
            let ptype = match r.get(2) {
                None => PinType::Passive,
                Some(t) => match PinType::parse(&t.to_string()) {
                    Some(t) => t,
                    None => return raise(st, ctx, Diagnostic::error("bad_pin", format!("`{t}` is not a pin type (pin {})", r[1]))),
                },
            };
            sides[i].push(Slot::Pin { numbers, name: r[1].to_string(), ptype });
        }
    }
    let num = |k: &str| layout.get(k).and_then(|v| v.as_int().ok()).unwrap_or(0) as i32;
    Ok(layout_symbol(&format!("wirx:{name}"), name, prefix, &sides, num("width"), num("height")))
}

fn rail(st: &Shared, ctx: &NativeCallContext, name: &str, kind: RailKind) -> RResult<Net> {
    let span = ctx_span(st, ctx);
    let r = st.borrow_mut().b.rail(name, kind, span);
    match r {
        Ok(node) => Ok(Net { node, name: name.into() }),
        Err(e) => raise(st, ctx, Diagnostic::error("bad_rail", e)),
    }
}

fn build_engine(st: &Shared, base_dir: &Path) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(50_000_000);
    // The optimizer inlines global constants (`input`, `left`, rails) even where a parameter or
    // captured variable of the same name should win; scripts are small, so skip it.
    e.set_optimization_level(rhai::OptimizationLevel::None);
    e.set_max_call_levels(64);
    // Pin tables are long literals; the defaults are tuned for small expressions.
    e.set_max_expr_depths(512, 512);
    e.on_print(|s| eprintln!("{s}"));
    e.on_debug(|s, _, _| eprintln!("{s}"));
    #[allow(deprecated)]
    e.register_debugger(|_, d| d, |_, _, _, _, _| Ok(rhai::debugger::DebuggerCommand::Continue));
    e.set_module_resolver(FileModuleResolver::new_with_path_and_extension(base_dir, "wx"));

    e.register_type_with_name::<Part>("Part").register_type_with_name::<PinRef>("Pin").register_type_with_name::<Net>("Net").register_type_with_name::<NoConnect>("NC");

    // Standard parts: Name(), Name(value), Name(value, footprint), Name(value, footprint, options).
    for p in stdlib::PARTS {
        let def = p.to_def();
        macro_rules! ctor {
            ($($a:ident),*) => {{
                let st = st.clone();
                let def = def.clone();
                e.register_fn(p.name, move |ctx: NativeCallContext, $($a: Dynamic),*| -> RResult<Part> { instantiate(&st, &ctx, def.clone(), &[$($a),*]) });
            }};
        }
        ctor!();
        ctor!(a);
        ctor!(a, b);
        ctor!(a, b, c);
    }
    for (fname, dual) in [("Conn", false), ("Conn2", true)] {
        macro_rules! conn {
            ($($a:ident),*) => {{
                let st = st.clone();
                e.register_fn(fname, move |ctx: NativeCallContext, n: i64, $($a: Dynamic),*| -> RResult<Part> {
                    let name = format!("Conn_{}x{:02}", if dual { "02" } else { "01" }, n);
                    match generated_part(&name) {
                        Some(def) => instantiate(&st, &ctx, def, &[Dynamic::UNIT, $($a),*]),
                        None => raise(&st, &ctx, Diagnostic::error("bad_connector", format!("connectors have 1 to 64 {}", if dual { "rows" } else { "pins" }))),
                    }
                });
            }};
        }
        conn!();
        conn!(a);
        conn!(a, b);
    }
    {
        let st = st.clone();
        e.register_fn("part", move |ctx: NativeCallContext, m: Map| custom_part(&st, &ctx, m));
    }

    // Pins.
    {
        let st = st.clone();
        e.register_indexer_get(move |ctx: NativeCallContext, p: &mut Part, k: ImmutableString| pin_lookup(&st, &ctx, p, &k));
    }
    {
        let st = st.clone();
        e.register_indexer_get(move |ctx: NativeCallContext, p: &mut Part, k: i64| pin_lookup(&st, &ctx, p, &k.to_string()));
    }
    {
        let st = st.clone();
        e.register_fn("pin", move |ctx: NativeCallContext, p: &mut Part, k: Dynamic| pin_lookup(&st, &ctx, p, &k.to_string()));
    }
    {
        let st = st.clone();
        e.register_fn("pins", move |p: &mut Part, sel: ImmutableString| -> Array {
            let s = st.borrow();
            let part = &s.b.design.parts[s.b.design.instances[p.id].part];
            let by_type = PinType::parse(&sel);
            let mut out = Array::new();
            let mut seen = HashSet::new();
            for (k, pin) in part.pins.iter().enumerate() {
                let hit = match by_type {
                    Some(t) => pin.ptype == t,
                    None => pin.name.starts_with(sel.as_str()),
                };
                if hit && seen.insert(pin.name.clone()) {
                    let ks: Vec<usize> = part.pins.iter().enumerate().filter(|(_, q)| q.name == pin.name).map(|(j, _)| j).collect();
                    out.push(Dynamic::from(PinRef { inst: p.id, pins: if by_type.is_some() { ks } else { vec![k] } }));
                }
            }
            out
        });
    }

    // Connections.
    e.register_custom_operator("->", 160).expect("`->` operator");
    {
        let st = st.clone();
        e.register_fn("->", move |ctx: NativeCallContext, a: Dynamic, b: Dynamic| link(&st, &ctx, a, b));
    }
    {
        let st = st.clone();
        e.register_fn("nc", move |ctx: NativeCallContext, x: Dynamic| -> RResult<()> { link(&st, &ctx, x, Dynamic::from(NoConnect)).map(|_| ()) });
    }

    // Nets and rails.
    {
        let st = st.clone();
        e.register_fn("net", move |ctx: NativeCallContext, name: ImmutableString| -> Net {
            let span = ctx_span(&st, &ctx);
            Net { node: st.borrow_mut().b.net(&name, span), name: name.to_string() }
        });
    }
    {
        let st = st.clone();
        e.register_fn("net", move || -> Net { Net { node: st.borrow_mut().b.new_node(), name: String::new() } });
    }
    for (fname, kind) in [("supply", RailKind::Supply), ("ground", RailKind::Ground), ("negative", RailKind::Negative)] {
        let st = st.clone();
        e.register_fn(fname, move |ctx: NativeCallContext, name: ImmutableString| rail(&st, &ctx, &name, kind));
    }
    {
        let st = st.clone();
        e.register_fn("driven", move |n: Net| -> Net {
            st.borrow_mut().b.flag(n.node);
            n
        });
    }

    // Sheets, groups and hints.
    {
        let st = st.clone();
        e.register_fn("sheet", move |ctx: NativeCallContext, name: ImmutableString, f: FnPtr| -> RResult<()> {
            let s = st.borrow_mut().b.add_sheet(&name);
            st.borrow_mut().sheet_stack.push(s);
            let r = f.call_within_context::<Dynamic>(&ctx, ());
            st.borrow_mut().sheet_stack.pop();
            r.map(|_| ())
        });
    }
    {
        let st = st.clone();
        e.register_fn("group", move |ctx: NativeCallContext, name: ImmutableString, f: FnPtr| -> RResult<()> {
            let g = {
                let mut s = st.borrow_mut();
                let sheet = s.current_sheet();
                s.b.add_group(name.to_string(), sheet, true)
            };
            st.borrow_mut().group_stack.push(g);
            let r = f.call_within_context::<Dynamic>(&ctx, ());
            st.borrow_mut().group_stack.pop();
            r.map(|_| ())
        });
    }
    {
        let st = st.clone();
        e.register_fn("near", move |a: Part, p: PinRef| {
            let mut s = st.borrow_mut();
            let g = s.b.design.instances[a.id].group;
            s.b.design.groups[g].near.push((a.id, p.inst, p.pins[0]));
        });
    }
    {
        let st = st.clone();
        e.register_fn("flow", move |ctx: NativeCallContext, dir: ImmutableString| -> RResult<()> {
            let flow = match dir.as_str() {
                "right" => 0,
                "up" => 90,
                "left" => 180,
                "down" => 270,
                _ => return raise(&st, &ctx, Diagnostic::error("bad_hint", format!("unknown flow direction `{dir}`")).hint("use right, down, left or up")),
            };
            let g = current_group(&st, &ctx);
            st.borrow_mut().b.design.groups[g].flow = flow;
            Ok(())
        });
    }
    // `version("1.2")` is another name for the revision.
    {
        let st = st.clone();
        e.register_fn("version", move |v: ImmutableString| {
            st.borrow_mut().b.design.meta.insert("revision".to_string(), v.to_string());
        });
    }
    for key in ["title", "revision", "author", "company", "date", "comment"] {
        let st = st.clone();
        e.register_fn(key, move |v: ImmutableString| {
            st.borrow_mut().b.design.meta.insert(key.to_string(), v.to_string());
        });
    }
    e.register_fn("e24", |v: f64| values::snap_series(v, 24));
    e.register_fn("e96", |v: f64| values::snap_series(v, 96));
    e.register_fn("e24", |v: i64| values::snap_series(v as f64, 24));
    e.register_fn("e96", |v: i64| values::snap_series(v as f64, 96));

    // Printing handles.
    e.register_fn("to_string", |p: &mut Part| format!("part #{}", p.id));
    e.register_fn("to_string", |p: &mut PinRef| format!("pin {}:{:?}", p.inst, p.pins));
    e.register_fn("to_string", |n: &mut Net| if n.name.is_empty() { "net".to_string() } else { format!("net {}", n.name) });

    // Constants: rails, pin types, sides, directions, NC. (Shapes are strings: `symbol: "npn"`.)
    // They are supplied by a variable resolver that only answers when no variable, parameter or
    // captured variable of that name exists, so user names always win.
    let mut constants: HashMap<String, Dynamic> = HashMap::new();
    {
        let mut s = st.borrow_mut();
        for (konst, name, kind) in stdlib::RAILS {
            let node = s.b.rail(name, *kind, None).expect("built-in rail");
            constants.insert(konst.to_string(), Dynamic::from(Net { node, name: name.to_string() }));
        }
    }
    for t in PinType::ALL {
        constants.insert(t.name().into(), t.name().into());
    }
    for name in ["left", "right", "top", "bottom", "up", "down", "hidden", "gap"] {
        constants.insert(name.into(), name.into());
    }
    constants.insert("NC".into(), Dynamic::from(NoConnect));
    #[allow(deprecated)]
    e.on_var(move |name, _, ctx| {
        if ctx.scope().contains(name) {
            return Ok(None);
        }
        Ok(constants.get(name).cloned())
    });
    e
}

/// Map a Rhai error to a diagnostic, with a hint for the common mistakes.
fn script_error(st: &Shared, err: &EvalAltResult) -> Diagnostic {
    // Innermost error and the source it happened in.
    let mut source: Option<String> = None;
    let mut cur = err;
    loop {
        match cur {
            EvalAltResult::ErrorInFunctionCall(_, src, inner, _) => {
                if !src.is_empty() {
                    source = Some(src.clone());
                }
                cur = inner;
            }
            EvalAltResult::ErrorInModule(path, inner, _) => {
                source = Some(path.clone());
                cur = inner;
            }
            _ => break,
        }
    }
    let span = st.borrow_mut().span(source.as_deref(), cur.position());
    let d = match cur {
        EvalAltResult::ErrorFunctionNotFound(sig, _) => {
            let name = sig.split(['(', ' ']).next().unwrap_or(sig).to_string();
            let s = st.borrow();
            let candidates: Vec<String> = s.natives.iter().cloned().collect();
            let mut d = Diagnostic::error("unknown_function", format!("no function `{name}` taking these arguments: {sig}"));
            if let Some(g) = suggest(&name, &candidates)
                && g != name
            {
                d = d.hint(format!("did you mean `{g}`?"));
            } else if stdlib::find(&name).is_some() {
                d = d.hint(format!("`{name}` takes (value, footprint, options), all optional"));
            } else {
                d = d.hint("define custom parts as `fn Name() { part(#{ … }) }`; see `wirx lib list` for the standard parts");
            }
            d
        }
        EvalAltResult::ErrorVariableNotFound(name, _) => {
            let rails: Vec<String> = stdlib::RAILS.iter().map(|r| r.0.to_string()).collect();
            let mut d = Diagnostic::error("unknown_variable", format!("no variable `{name}` here"));
            if let Some(g) = suggest(name, &rails) {
                d = d.hint(format!("did you mean the rail `{g}`?"));
            } else if auto_rail(name).is_some() || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                d = d.hint(format!("rail names are written `supply(\"{name}\")`, or use a built-in like `P3V3`"));
            } else {
                d = d.hint("functions cannot see variables from outside: pass them in as parameters, or use `net(\"NAME\")`");
            }
            d
        }
        EvalAltResult::ErrorParsing(kind, _) => Diagnostic::error("syntax", kind.to_string()),
        EvalAltResult::ErrorTooManyOperations(_) => Diagnostic::error("too_much_work", "the script ran too long (an endless loop?)"),
        other => Diagnostic::error("script", other.to_string()),
    };
    d.at_opt(span)
}

/// Run a design script and build the design.
pub fn run(name: &str, text: &str, base_dir: &Path) -> (Sources, Option<Design>, Vec<Diagnostic>) {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("design").to_string();
    let mut sources = Sources::default();
    let main = sources.add(name, text);
    let st: Shared = Rc::new(RefCell::new(State {
        b: Builder::new(&stem),
        base_dir: base_dir.to_path_buf(),
        libraries: HashMap::new(),
        sources,
        source_ids: HashMap::from([(name.to_string(), main)]),
        main_source: name.to_string(),
        design_name: stem,
        sheet_stack: vec![],
        group_stack: vec![],
        root_groups: HashMap::new(),
        fn_groups: HashMap::new(),
        fn_group_names: HashMap::new(),
        fn_group_set: HashSet::new(),
        natives: HashSet::new(),
        pending: None,
    }));
    let engine = build_engine(&st, base_dir);
    let natives: HashSet<String> = engine.gen_fn_signatures(true).iter().map(|s| s.split('(').next().unwrap_or("").trim().to_string()).collect();
    st.borrow_mut().natives = natives;

    let result = match engine.compile(text) {
        Err(e) => {
            let span = st.borrow_mut().span(None, e.position());
            Err(Diagnostic::error("syntax", e.err_type().to_string()).at_opt(span))
        }
        Ok(mut ast) => {
            ast.set_source(name);
            engine.run_ast_with_scope(&mut Scope::new(), &ast).map_err(|err| {
                let pending = st.borrow_mut().pending.take();
                pending.unwrap_or_else(|| script_error(&st, &err))
            })
        }
    };
    drop(engine);
    let state = Rc::try_unwrap(st).ok().expect("engine dropped").into_inner();
    let State { mut b, sources, fn_group_set, root_groups, .. } = state;
    if let Err(d) = result {
        let mut diags = b.diags;
        diags.push(d);
        return (sources, None, diags);
    }
    // A function that placed a single part (e.g. a custom part's constructor) is not a group:
    // its part joins the sheet's loose parts.
    let mut root_groups = root_groups;
    for g in 0..b.design.groups.len() {
        if fn_group_set.contains(&g) && b.design.groups[g].instances.len() == 1 {
            let inst = b.design.groups[g].instances[0];
            let sheet = b.design.groups[g].sheet;
            let root = *root_groups.entry(sheet).or_insert_with(|| {
                let name = b.design.sheets[sheet].name.clone();
                b.add_group(name, sheet, false)
            });
            b.design.groups[g].instances.clear();
            b.design.groups[root].instances.push(inst);
            b.design.instances[inst].group = root;
            // Its placement hints go along.
            let near = std::mem::take(&mut b.design.groups[g].near);
            let chained = std::mem::take(&mut b.design.groups[g].chained);
            b.design.groups[root].near.extend(near);
            b.design.groups[root].chained.extend(chained);
        }
    }
    let (design, diags) = b.finish();
    (sources, Some(design), diags)
}
