//! Semantic analysis: load files, resolve names, expand modules and build the flat [`Design`].

use crate::diag::{Diagnostic, Sources, Span};
use crate::ir::*;
use crate::symbol::boxgen::{box_symbol, BoxPin, Side};
use crate::symbol::templates::{self, connector, connector_dual, from_template, TEMPLATE_NAMES};
use crate::symbol::{PinType, RailKind, SymbolDef};
use crate::syntax::ast::{self, ArgValue, Endpoint, Hint, Item, Stmt};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const STD_SOURCE: &str = include_str!("../../lib/std.wx");

/// Parsed files: the standard library first, then the main file and everything it `use`s.
pub struct Program {
    pub files: Vec<(usize, ast::File)>,
    pub main_file: usize,
}

/// Load the standard library, a main source and its `use "…"` imports.
pub fn load(sources: &mut Sources, main_name: &str, main_text: &str, base_dir: &Path) -> Result<Program, Vec<Diagnostic>> {
    let mut files = Vec::new();
    let std_id = sources.add("<std>/std.wx", STD_SOURCE);
    let std_ast = crate::syntax::parser::parse(std_id, STD_SOURCE).map_err(|e| vec![e])?;
    files.push((std_id, std_ast));

    let main_id = sources.add(main_name, main_text);
    let main_ast = crate::syntax::parser::parse(main_id, main_text).map_err(|e| vec![e])?;
    let mut queue = vec![(main_id, main_ast, base_dir.to_path_buf())];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut diags = Vec::new();
    while let Some((id, file, dir)) = queue.pop() {
        for item in &file.items {
            if let Item::Use(u) = item
                && let Some(f) = &u.file
            {
                let path = dir.join(&f.text);
                let canon = path.canonicalize().unwrap_or(path.clone());
                if !seen.insert(canon.clone()) {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let fid = sources.add(path.display().to_string(), text.clone());
                        match crate::syntax::parser::parse(fid, &text) {
                            Ok(ast) => queue.push((fid, ast, canon.parent().map(Path::to_path_buf).unwrap_or_default())),
                            Err(e) => diags.push(e),
                        }
                    }
                    Err(e) => diags.push(Diagnostic::error("use_not_found", format!("cannot read `{}`: {e}", path.display())).at(f.span)),
                }
            }
        }
        files.push((id, file));
    }
    if diags.is_empty() { Ok(Program { files, main_file: main_id }) } else { Err(diags) }
}

/// Rails that need no declaration: names starting with `+` or `-` (`+3V3`, `-12V`).
pub fn auto_rail(name: &str) -> Option<RailKind> {
    if name.starts_with('+') {
        Some(RailKind::Supply)
    } else if name.starts_with('-') {
        Some(RailKind::Negative)
    } else {
        None
    }
}

fn metric(size: &str) -> Option<&'static str> {
    Some(match size {
        "0201" => "0603Metric",
        "0402" => "1005Metric",
        "0603" => "1608Metric",
        "0805" => "2012Metric",
        "1206" => "3216Metric",
        "1210" => "3225Metric",
        "2010" => "5025Metric",
        "2512" => "6332Metric",
        _ => return None,
    })
}

/// Expand `0805` into a footprint for the part's symbol family.
fn footprint_alias(symbol: &str, size: &str) -> Option<String> {
    let m = metric(size)?;
    let (lib, pre) = match symbol {
        "resistor" => ("Resistor_SMD", "R"),
        "capacitor" => ("Capacitor_SMD", "C"),
        "inductor" => ("Inductor_SMD", "L"),
        "led" => ("LED_SMD", "LED"),
        "fuse" => ("Fuse", "Fuse"),
        "diode" | "schottky" | "zener" => ("Diode_SMD", "D"),
        _ => return None,
    };
    Some(format!("{lib}:{pre}_{size}_{m}"))
}

/// Closest candidate by edit distance, for "did you mean" hints.
pub fn suggest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a String>) -> Option<String> {
    fn dist(a: &str, b: &str) -> usize {
        let a: Vec<char> = a.to_lowercase().chars().collect();
        let b: Vec<char> = b.to_lowercase().chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() {
                cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
            }
            prev = cur;
        }
        prev[b.len()]
    }
    candidates
        .into_iter()
        .map(|c| (dist(word, c), c))
        // Very short names are too close to everything to guess at.
        .filter(|(d, c)| *d <= (word.len().max(c.len()) / 3).max(usize::from(word.len() >= 3)))
        .min_by_key(|(d, c)| (*d, (*c).clone()))
        .map(|(_, c)| c.clone())
}

/// Nodes of a pin reference, and the (instance, pin) pairs behind them.
type PinNodes = (Vec<usize>, Vec<(usize, usize)>);

#[derive(Default)]
struct NodeInfo {
    /// (priority, name, span): rail 4, sheet-level name 3, module-local 2, open port 1.
    names: Vec<(u8, String, Option<Span>)>,
    flagged: bool,
}

#[derive(Clone)]
enum Bound {
    Part(usize),
    Module { module: String, ports: HashMap<String, usize> },
}

struct Scope {
    module: bool,
    path: String,
    ports: HashMap<String, usize>,
    locals: HashMap<String, usize>,
    insts: HashMap<String, Bound>,
    group: usize,
    sheet: usize,
}

struct Elab<'a> {
    parts_ast: HashMap<String, &'a ast::PartDecl>,
    modules: HashMap<String, &'a ast::ModuleDecl>,
    rails: HashMap<String, RailKind>,
    design: Design,
    diags: Vec<Diagnostic>,
    part_cache: HashMap<String, usize>,
    parent: Vec<usize>,
    nodes: Vec<NodeInfo>,
    pin_nodes: Vec<Vec<usize>>,
    globals: HashMap<String, usize>,
    stack: Vec<String>,
    explicit_refs: Vec<Option<(String, Span)>>,
}

impl<'a> Elab<'a> {
    fn new_node(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.nodes.push(NodeInfo::default());
        self.parent.len() - 1
    }
    fn find(&mut self, mut a: usize) -> usize {
        while self.parent[a] != a {
            self.parent[a] = self.parent[self.parent[a]];
            a = self.parent[a];
        }
        a
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
            let moved = std::mem::take(&mut self.nodes[ra]);
            self.nodes[rb].names.extend(moved.names);
            self.nodes[rb].flagged |= moved.flagged;
        }
    }
    fn named_node(&mut self, prio: u8, name: String, span: Option<Span>) -> usize {
        let n = self.new_node();
        self.nodes[n].names.push((prio, name, span));
        n
    }

    fn rail_kind(&self, name: &str) -> Option<RailKind> {
        self.rails.get(name).copied().or_else(|| auto_rail(name))
    }

    // ------------------------------------------------------------------ parts

    fn resolve_part(&mut self, name: &str) -> Option<usize> {
        if let Some(&i) = self.part_cache.get(name) {
            return Some(i);
        }
        let def = if let Some(decl) = self.parts_ast.get(name).copied() {
            self.convert_part(decl)
        } else { generated_part(name)? };
        let sym = build_symbol(&def);
        self.design.parts.push(def);
        self.design.symbols.push(sym);
        let i = self.design.parts.len() - 1;
        self.part_cache.insert(name.to_string(), i);
        Some(i)
    }

    fn convert_part(&mut self, d: &ast::PartDecl) -> PartDef {
        let mut def = PartDef {
            name: d.name.text.clone(),
            prefix: "U".into(),
            footprint: String::new(),
            value: d.name.text.clone(),
            description: String::new(),
            datasheet: String::new(),
            mpn: String::new(),
            fields: d.fields.iter().map(|(k, v)| (k.text.clone(), v.text.clone())).collect(),
            symbol: "box".into(),
            pins: Vec::new(),
            open_pins_ok: false,
            span: Some(d.name.span),
        };
        for (k, v) in &d.props {
            let val = v.as_ref().map(|v| v.text.clone()).unwrap_or_default();
            match k.text.as_str() {
                "prefix" => def.prefix = val,
                "footprint" => def.footprint = val,
                "value" => def.value = val,
                "description" => def.description = val,
                "datasheet" => def.datasheet = val,
                "mpn" => def.mpn = val,
                "open_pins_ok" => def.open_pins_ok = true,
                "symbol" => {
                    if val != "box" && val != "conn" && templates::template(&val).is_none() {
                        let hint = suggest(&val, &TEMPLATE_NAMES.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                            .map(|s| format!("did you mean `{s}`?"))
                            .unwrap_or_else(|| format!("available: {}", TEMPLATE_NAMES.join(", ")));
                        self.diags.push(Diagnostic::error("unknown_symbol", format!("unknown symbol shape `{val}`")).at(v.as_ref().unwrap().span).hint(hint));
                    } else {
                        def.symbol = val;
                    }
                }
                other => self.diags.push(
                    Diagnostic::error("unknown_property", format!("unknown part property `{other}`"))
                        .at(k.span)
                        .hint("properties: prefix, footprint, value, description, datasheet, mpn, symbol, open_pins_ok, field, pins"),
                ),
            }
        }
        let mut numbers = HashSet::new();
        for p in &d.pins {
            let ptype = match &p.ptype {
                None => PinType::Passive,
                Some(t) => PinType::parse(&t.text).unwrap_or_else(|| {
                    self.diags.push(Diagnostic::error("unknown_pin_type", format!("unknown pin type `{}`", t.text)).at(t.span).hint(
                        "types: input, output, bidirectional, tri_state, passive, power_in, power_out, open_collector, open_emitter, free, unspecified, no_connect",
                    ));
                    PinType::Passive
                }),
            };
            let side = p.side.as_ref().and_then(|s| {
                let r = Side::parse(&s.text);
                if r.is_none() {
                    self.diags.push(Diagnostic::error("unknown_side", format!("unknown side `{}`", s.text)).at(s.span).hint("sides: left, right, top, bottom"));
                }
                r
            });
            for n in &p.numbers {
                if !numbers.insert(n.text.clone()) {
                    self.diags.push(Diagnostic::error("duplicate_pin", format!("pin number `{}` is declared twice in part `{}`", n.text, d.name.text)).at(n.span));
                }
                def.pins.push(PartPin { number: n.text.clone(), name: p.name.text.clone(), ptype, side, hidden: p.hidden });
            }
        }
        if def.pins.is_empty() {
            self.diags.push(Diagnostic::error("part_without_pins", format!("part `{}` declares no pins", d.name.text)).at(d.name.span).hint("add `pins { 1 A  2 B }`"));
        }
        if let Some(t) = templates::template(&def.symbol)
            && t.slots.len() != def.pins.len()
        {
            self.diags.push(
                Diagnostic::error(
                    "template_pin_count",
                    format!("symbol `{}` has {} pins ({}), but part `{}` declares {}", def.symbol, t.slots.len(), t.slot_names, d.name.text, def.pins.len()),
                )
                .at(d.name.span)
                .hint(format!("declare the pins in the order {}", t.slot_names)),
            );
        }
        def
    }

    // ------------------------------------------------------------------ names

    fn net_node(&mut self, scope: &mut Scope, name: &ast::Name) -> Option<usize> {
        let text = &name.text;
        if let Some(b) = scope.insts.get(text) {
            let what = match b {
                Bound::Part(_) => "a part",
                Bound::Module { .. } => "a module instance",
            };
            self.diags.push(
                Diagnostic::error("instance_as_net", format!("`{text}` is {what}, not a net"))
                    .at(name.span)
                    .hint(format!("connect one of its pins or ports, e.g. `{text}.1`")),
            );
            return None;
        }
        if scope.module
            && let Some(&n) = scope.ports.get(text)
        {
            return Some(n);
        }
        if self.rail_kind(text).is_some() {
            if let Some(&n) = self.globals.get(text) {
                return Some(n);
            }
            let n = self.named_node(4, text.clone(), Some(name.span));
            self.globals.insert(text.clone(), n);
            return Some(n);
        }
        if !scope.module {
            if let Some(&n) = self.globals.get(text) {
                return Some(n);
            }
            let n = self.named_node(3, text.clone(), Some(name.span));
            self.globals.insert(text.clone(), n);
            return Some(n);
        }
        if let Some(&n) = scope.locals.get(text) {
            return Some(n);
        }
        let n = self.named_node(2, format!("{}/{}", scope.path, text), Some(name.span));
        scope.locals.insert(text.clone(), n);
        Some(n)
    }

    /// Nodes of `inst.pin`, plus the (instance, pin) pairs when it is a part pin.
    fn pin_nodes(&mut self, scope: &Scope, inst: &ast::Name, pin: &ast::Name) -> Option<PinNodes> {
        match scope.insts.get(&inst.text) {
            Some(Bound::Part(i)) => {
                let i = *i;
                let part = &self.design.parts[self.design.instances[i].part];
                let mut idx: Vec<usize> = part.pins.iter().enumerate().filter(|(_, p)| p.number == pin.text).map(|(k, _)| k).collect();
                if idx.is_empty() {
                    idx = part.pins.iter().enumerate().filter(|(_, p)| p.name == pin.text).map(|(k, _)| k).collect();
                }
                if idx.is_empty() {
                    let names: Vec<String> = part.pins.iter().flat_map(|p| [p.number.clone(), p.name.clone()]).collect();
                    let list: Vec<String> = part
                        .pins
                        .iter()
                        .map(|p| if p.name == "~" || p.name == p.number { p.number.clone() } else { format!("{} {}", p.number, p.name) })
                        .collect();
                    let hint = match suggest(&pin.text, &names) {
                        Some(s) => format!("did you mean `{}.{s}`? pins: {}", inst.text, list.join(", ")),
                        None => format!("pins: {}", list.join(", ")),
                    };
                    self.diags.push(
                        Diagnostic::error("unknown_pin", format!("part `{}` ({}) has no pin `{}`", part.name, inst.text, pin.text)).at(pin.span).hint(hint),
                    );
                    return None;
                }
                let nodes = idx.iter().map(|&k| self.pin_nodes[i][k]).collect();
                Some((nodes, idx.into_iter().map(|k| (i, k)).collect()))
            }
            Some(Bound::Module { module, ports }) => match ports.get(&pin.text) {
                Some(&n) => Some((vec![n], vec![])),
                None => {
                    let hint = format!("ports of `{module}`: {}", ports.keys().cloned().collect::<Vec<_>>().join(", "));
                    self.diags.push(Diagnostic::error("unknown_port", format!("module `{module}` ({}) has no port `{}`", inst.text, pin.text)).at(pin.span).hint(hint));
                    None
                }
            },
            None => {
                let hint = suggest(&inst.text, scope.insts.keys()).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("declare it first: `{} = R(10k)`", inst.text));
                self.diags.push(Diagnostic::error("unknown_instance", format!("no part or module instance named `{}` here", inst.text)).at(inst.span).hint(hint));
                None
            }
        }
    }

    // ------------------------------------------------------------------ bodies

    fn new_group(&mut self, name: String, sheet: usize, framed: bool) -> usize {
        self.design.groups.push(Group { name, sheet, instances: vec![], near: vec![], flow: 0, framed });
        self.design.sheets[sheet].groups.push(self.design.groups.len() - 1);
        self.design.groups.len() - 1
    }

    fn body(&mut self, scope: &mut Scope, body: &'a [Stmt]) {
        // Pass 1: instances, so connections may refer to parts declared further down.
        for s in body {
            if let Stmt::Inst { name, ty, args, span } = s {
                if scope.insts.contains_key(&name.text) {
                    self.diags.push(Diagnostic::error("duplicate_instance", format!("`{}` is already declared here", name.text)).at(name.span));
                    continue;
                }
                if let Some(&m) = self.modules.get(&ty.text) {
                    self.module_instance(scope, name, m, args, *span);
                } else if let Some(p) = self.resolve_part(&ty.text) {
                    self.part_instance(scope, name, p, args, *span);
                } else {
                    let mut names: Vec<String> = self.parts_ast.keys().cloned().collect();
                    names.extend(self.modules.keys().cloned());
                    let hint = suggest(&ty.text, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| {
                        format!("declare it with `part {} {{ … }}` or `module {}(…) {{ … }}`, or check `wirx lib list`", ty.text, ty.text)
                    });
                    self.diags.push(Diagnostic::error("unknown_type", format!("unknown part or module `{}`", ty.text)).at(ty.span).hint(hint));
                }
            }
        }
        // Pass 2: connections and annotations.
        for s in body {
            match s {
                Stmt::Inst { .. } => {}
                Stmt::Connect { groups, .. } => self.connect(scope, groups),
                Stmt::Nc { pins, .. } => {
                    for e in pins {
                        self.mark_nc(scope, e);
                    }
                }
                Stmt::PowerFlag { nets, .. } => {
                    for n in nets {
                        if let Some(node) = self.net_node(scope, n) {
                            let r = self.find(node);
                            self.nodes[r].flagged = true;
                        }
                    }
                }
                Stmt::Layout { hints, .. } => {
                    for h in hints {
                        self.hint(scope, h);
                    }
                }
            }
        }
    }

    fn part_instance(&mut self, scope: &mut Scope, name: &ast::Name, p: usize, args: &[ast::Arg], span: Span) {
        let part = self.design.parts[p].clone();
        let mut value = part.value.clone();
        let mut footprint = part.footprint.clone();
        let mut fields = part.fields.clone();
        if !part.mpn.is_empty() {
            fields.push(("MPN".into(), part.mpn.clone()));
        }
        if !part.datasheet.is_empty() {
            fields.push(("Datasheet".into(), part.datasheet.clone()));
        }
        let mut dnp = false;
        let mut explicit_ref = None;
        let mut positional = 0;
        for a in args {
            let ArgValue::Lit(v) = &a.value else {
                self.diags.push(Diagnostic::error("bad_argument", "parts take values, not pins").at(a.key.as_ref().map(|k| k.span).unwrap_or(span)));
                continue;
            };
            let key = match &a.key {
                Some(k) => k.text.clone(),
                None => {
                    positional += 1;
                    match positional {
                        1 => "value".into(),
                        2 => "footprint".into(),
                        _ => {
                            self.diags.push(
                                Diagnostic::error("bad_argument", "too many positional arguments")
                                    .at(v.span)
                                    .hint("parts take `(value, footprint)`; name the others, e.g. `mpn: \"RC0603FR-0710KL\"`"),
                            );
                            continue;
                        }
                    }
                }
            };
            match key.as_str() {
                "value" => value = v.text.clone(),
                "footprint" | "fp" => {
                    if v.text.contains(':') {
                        footprint = v.text.clone();
                    } else if let Some(f) = footprint_alias(&part.symbol, &v.text) {
                        footprint = f;
                    } else {
                        self.diags.push(
                            Diagnostic::error("unknown_footprint", format!("`{}` is not a footprint alias for `{}`", v.text, part.name))
                                .at(v.span)
                                .hint("give a full footprint name like \"Package_SO:SOIC-8_3.9x4.9mm_P1.27mm\""),
                        );
                    }
                }
                "ref" => explicit_ref = Some((v.text.clone(), v.span)),
                "dnp" => dnp = matches!(v.text.as_str(), "yes" | "true" | "1"),
                "mpn" => {
                    fields.retain(|f| f.0 != "MPN");
                    fields.push(("MPN".into(), v.text.clone()));
                }
                other => {
                    fields.retain(|f| f.0 != other);
                    fields.push((other.to_string(), v.text.clone()));
                }
            }
        }
        let npins = part.pins.len();
        let id = self.design.instances.len();
        self.design.instances.push(Instance {
            path: format!("{}/{}", scope.path, name.text),
            local: name.text.clone(),
            part: p,
            reference: String::new(),
            value,
            footprint,
            fields,
            dnp,
            group: scope.group,
            sheet: scope.sheet,
            span: name.span,
            pin_net: vec![None; npins],
            nc: vec![false; npins],
        });
        self.explicit_refs.push(explicit_ref);
        let nodes = (0..npins).map(|_| self.new_node()).collect();
        self.pin_nodes.push(nodes);
        self.design.groups[scope.group].instances.push(id);
        scope.insts.insert(name.text.clone(), Bound::Part(id));
    }

    fn module_instance(&mut self, scope: &mut Scope, name: &ast::Name, m: &'a ast::ModuleDecl, args: &[ast::Arg], span: Span) {
        if self.stack.contains(&m.name.text) {
            self.diags.push(Diagnostic::error("recursive_module", format!("module `{}` instantiates itself", m.name.text)).at(span));
            return;
        }
        let path = format!("{}/{}", scope.path, name.text);
        let mut ports: HashMap<String, usize> = HashMap::new();
        let mut positional = 0;
        for a in args {
            let port = match &a.key {
                Some(k) => {
                    if !m.params.iter().any(|p| p.text == k.text) {
                        let names: Vec<String> = m.params.iter().map(|p| p.text.clone()).collect();
                        let hint = suggest(&k.text, &names).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| format!("ports: {}", names.join(", ")));
                        self.diags.push(Diagnostic::error("unknown_port", format!("module `{}` has no port `{}`", m.name.text, k.text)).at(k.span).hint(hint));
                        continue;
                    }
                    k.text.clone()
                }
                None => {
                    positional += 1;
                    match m.params.get(positional - 1) {
                        Some(p) => p.text.clone(),
                        None => {
                            self.diags.push(Diagnostic::error("bad_argument", format!("module `{}` has only {} ports", m.name.text, m.params.len())).at(span));
                            continue;
                        }
                    }
                }
            };
            let node = match &a.value {
                ArgValue::Lit(v) if v.text.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                    self.diags.push(Diagnostic::error("bad_argument", format!("`{}` is not a net", v.text)).at(v.span).hint("module ports connect to nets, e.g. `vin: VBUS`"));
                    continue;
                }
                ArgValue::Lit(v) => self.net_node(scope, v),
                ArgValue::Pin(Endpoint::Pin { inst, pin }) => {
                    self.pin_nodes(scope, inst, pin).and_then(|(nodes, _)| {
                        for w in nodes.windows(2) {
                            self.union(w[0], w[1]);
                        }
                        nodes.first().copied()
                    })
                }
                ArgValue::Pin(_) => None,
            };
            if let Some(n) = node {
                ports.insert(port, n);
            }
        }
        for p in &m.params {
            if !ports.contains_key(&p.text) {
                let n = self.named_node(1, format!("{path}/{}", p.text), Some(p.span));
                ports.insert(p.text.clone(), n);
            }
        }
        // A module placed on a sheet becomes a framed group; one nested in another module is drawn
        // as part of its parent's group, next to the pins it connects to.
        let group = if scope.module { scope.group } else { self.new_group(format!("{} {}", m.name.text, name.text), scope.sheet, true) };
        let mut inner = Scope { module: true, path, ports: ports.clone(), locals: HashMap::new(), insts: HashMap::new(), group, sheet: scope.sheet };
        self.stack.push(m.name.text.clone());
        self.body(&mut inner, &m.body);
        self.stack.pop();
        scope.insts.insert(name.text.clone(), Bound::Module { module: m.name.text.clone(), ports });
    }

    fn connect(&mut self, scope: &mut Scope, groups: &[Vec<Endpoint>]) {
        let has_nc = groups.iter().flatten().any(|e| matches!(e, Endpoint::Nc(_)));
        if has_nc {
            for e in groups.iter().flatten() {
                if !matches!(e, Endpoint::Nc(_)) {
                    self.mark_nc(scope, e);
                }
            }
            return;
        }
        let mut nodes = Vec::new();
        for e in groups.iter().flatten() {
            match e {
                Endpoint::Net(n) => nodes.extend(self.net_node(scope, n)),
                Endpoint::Pin { inst, pin } => {
                    if let Some((ns, _)) = self.pin_nodes(scope, inst, pin) {
                        nodes.extend(ns);
                    }
                }
                Endpoint::Nc(_) => {}
            }
        }
        for w in nodes.windows(2) {
            self.union(w[0], w[1]);
        }
    }

    fn mark_nc(&mut self, scope: &mut Scope, e: &Endpoint) {
        match e {
            Endpoint::Pin { inst, pin } => {
                if let Some((_, pins)) = self.pin_nodes(scope, inst, pin) {
                    if pins.is_empty() {
                        self.diags.push(Diagnostic::error("bad_nc", "only part pins can be marked `nc`").at(e.span()));
                    }
                    for (i, k) in pins {
                        self.design.instances[i].nc[k] = true;
                    }
                }
            }
            _ => self.diags.push(Diagnostic::error("bad_nc", "only part pins can be marked `nc`").at(e.span()).hint("write `nc u.5` or `u.5 -- nc`")),
        }
    }

    fn hint(&mut self, scope: &mut Scope, h: &Hint) {
        match h {
            Hint::Flow(d) => {
                let flow = match d.text.as_str() {
                    "right" => 0,
                    "up" => 90,
                    "left" => 180,
                    "down" => 270,
                    _ => {
                        self.diags.push(Diagnostic::error("bad_hint", format!("unknown flow direction `{}`", d.text)).at(d.span).hint("use right, down, left or up"));
                        return;
                    }
                };
                self.design.groups[scope.group].flow = flow;
            }
            Hint::Near { inst, target } => {
                let Some(Bound::Part(a)) = scope.insts.get(&inst.text).cloned() else {
                    self.diags.push(Diagnostic::error("bad_hint", format!("`{}` is not a part in this block", inst.text)).at(inst.span));
                    return;
                };
                let Endpoint::Pin { inst: ti, pin } = target else {
                    self.diags.push(Diagnostic::error("bad_hint", "`near` needs a pin, e.g. `c1 near u.VDD`").at(target.span()));
                    return;
                };
                if let Some((_, pins)) = self.pin_nodes(scope, ti, pin)
                    && let Some(&(b, k)) = pins.first()
                {
                    self.design.groups[scope.group].near.push((a, b, k));
                }
            }
        }
    }

    // ------------------------------------------------------------------ finish

    fn finish(&mut self) {
        // Nets from pin nodes.
        let mut by_root: HashMap<usize, usize> = HashMap::new();
        for i in 0..self.design.instances.len() {
            for k in 0..self.pin_nodes[i].len() {
                let root = self.find(self.pin_nodes[i][k]);
                let n = *by_root.entry(root).or_insert_with(|| {
                    self.design.nets.push(Net { name: String::new(), kind: NetKind::Signal, pins: vec![], flagged: false, named: false, span: None });
                    self.design.nets.len() - 1
                });
                self.design.nets[n].pins.push((i, k));
                self.design.instances[i].pin_net[k] = Some(n);
            }
        }
        for (&root, &n) in &by_root {
            let info = &self.nodes[root];
            let mut names = info.names.clone();
            names.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(&b.1)));
            names.dedup_by(|a, b| a.1 == b.1);
            let rails: Vec<&(u8, String, Option<Span>)> = names.iter().filter(|x| x.0 == 4).collect();
            if rails.len() > 1 {
                self.diags.push(
                    Diagnostic::error("rails_shorted", format!("rails `{}` and `{}` are connected together", rails[0].1, rails[1].1)).at_opt(rails[1].2),
                );
            }
            let net = &mut self.design.nets[n];
            net.flagged = info.flagged;
            if let Some((_, name, span)) = names.first() {
                net.name = name.clone();
                net.named = true;
                net.span = *span;
                if let Some(kind) = self.rails.get(name).copied().or_else(|| auto_rail(name)) {
                    net.kind = NetKind::Rail(kind);
                }
            }
        }
        // Flagged rails that no pin uses still need nothing; flags on unknown nets are ignored.

        // References: explicit first, then numbered in declaration order per prefix.
        let mut used: HashSet<String> = HashSet::new();
        for (i, r) in self.explicit_refs.clone().into_iter().enumerate() {
            if let Some((r, span)) = r {
                if !used.insert(r.clone()) {
                    self.diags.push(Diagnostic::error("duplicate_reference", format!("reference `{r}` is used twice")).at(span));
                }
                self.design.instances[i].reference = r;
            }
        }
        let mut next: HashMap<String, u32> = HashMap::new();
        for i in 0..self.design.instances.len() {
            if !self.design.instances[i].reference.is_empty() {
                continue;
            }
            let prefix = self.design.parts[self.design.instances[i].part].prefix.clone();
            let n = next.entry(prefix.clone()).or_insert(1);
            while used.contains(&format!("{prefix}{n}")) {
                *n += 1;
            }
            let r = format!("{prefix}{n}");
            used.insert(r.clone());
            self.design.instances[i].reference = r;
        }

        // Unnamed nets: named after the most telling pin (`U1_XTAL1`): a named pin on the part with
        // the most pins. Without one, KiCad style `Net-(R1-Pad1)` from the first pin by reference.
        let mut taken: HashSet<String> = self.design.nets.iter().filter(|n| n.named).map(|n| n.name.clone()).collect();
        for n in 0..self.design.nets.len() {
            if self.design.nets[n].named {
                continue;
            }
            let best = self.design.nets[n]
                .pins
                .iter()
                .filter(|&&(i, k)| {
                    let p = &self.design.parts[self.design.instances[i].part];
                    let name = &p.pins[k].name;
                    p.pins.len() > 2 && !name.is_empty() && name != "~" && *name != p.pins[k].number
                })
                .max_by_key(|&&(i, _)| (self.design.parts[self.design.instances[i].part].pins.len(), std::cmp::Reverse(i)))
                .copied();
            if let Some((i, k)) = best {
                let pin = &self.design.parts[self.design.instances[i].part].pins[k].name;
                let clean: String = pin.chars().map(|c| if c.is_ascii_alphanumeric() || c == '+' || c == '-' { c } else { '_' }).collect();
                let name = format!("{}_{}", self.design.instances[i].reference, clean.trim_matches('_'));
                if taken.insert(name.clone()) {
                    self.design.nets[n].name = name;
                    continue;
                }
            }
            let (i, k) = *self.design.nets[n]
                .pins
                .iter()
                .min_by(|a, b| natural_cmp(&self.design.instances[a.0].reference, &self.design.instances[b.0].reference).then(a.1.cmp(&b.1)))
                .unwrap();
            let pad = &self.design.parts[self.design.instances[i].part].pins[k].number;
            self.design.nets[n].name = format!("Net-({}-Pad{pad})", self.design.instances[i].reference);
        }
        self.design.nets.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        for (n, net) in self.design.nets.iter().enumerate() {
            for &(i, k) in &net.pins {
                self.design.instances[i].pin_net[k] = Some(n);
            }
        }
    }
}

/// Compare strings with embedded numbers in numeric order (R2 < R10).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut text = String::new();
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                if !num.is_empty() {
                    out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                    num.clear();
                }
                text.push(c);
            }
        }
        out.push((text, num.parse().unwrap_or(0)));
        out
    };
    split(a).cmp(&split(b))
}

/// Parametric parts that need no declaration: `Conn_01x04`, `Conn_02x05`.
fn generated_part(name: &str) -> Option<PartDef> {
    let (rows, dual) = if let Some(n) = name.strip_prefix("Conn_01x") {
        (n.parse::<usize>().ok()?, false)
    } else {
        let n = name.strip_prefix("Conn_02x")?;
        (n.parse::<usize>().ok()?, true)
    };
    if rows == 0 || rows > 64 {
        return None;
    }
    let count = if dual { rows * 2 } else { rows };
    Some(PartDef {
        name: name.into(),
        prefix: "J".into(),
        footprint: format!(
            "Connector_PinHeader_2.54mm:PinHeader_{}x{:02}_P2.54mm_Vertical",
            if dual { 2 } else { 1 },
            rows
        ),
        value: name.into(),
        description: format!("Generic connector, {count} pins"),
        datasheet: String::new(),
        mpn: String::new(),
        fields: vec![],
        symbol: if dual { "conn2".into() } else { "conn".into() },
        pins: (1..=count)
            .map(|i| PartPin { number: i.to_string(), name: format!("Pin_{i}"), ptype: PinType::Passive, side: None, hidden: false })
            .collect(),
        open_pins_ok: false,
        span: None,
    })
}

/// The symbol drawn for a part.
pub fn build_symbol(def: &PartDef) -> SymbolDef {
    let id = format!("wirx:{}", def.name);
    let pins: Vec<(String, String, PinType, bool)> = def.pins.iter().map(|p| (p.number.clone(), p.name.clone(), p.ptype, p.hidden)).collect();
    let mut sym = match def.symbol.as_str() {
        "conn" | "conn2" => {
            let dual = def.symbol == "conn2";
            let t = if dual { connector_dual(def.pins.len().div_ceil(2)) } else { connector(def.pins.len()) };
            let mut s = from_template(&id, &def.name, &def.prefix, t, &pins);
            s.hide_pin_names = Some(true);
            s
        }
        name => match templates::template(name) {
            Some(t) if t.slots.len() == pins.len() => from_template(&id, &def.name, &def.prefix, t, &pins),
            _ => box_symbol(
                &id,
                &def.name,
                &def.prefix,
                &def
                    .pins
                    .iter()
                    .map(|p| BoxPin { number: p.number.clone(), name: p.name.clone(), ptype: p.ptype, side: p.side, hidden: p.hidden })
                    .collect::<Vec<_>>(),
            ),
        },
    };
    sym.description = (!def.description.is_empty()).then(|| def.description.clone());
    sym.open_pins_ok = def.open_pins_ok.then_some(true);
    for f in &mut sym.fields {
        match f.name.as_str() {
            "Value" => f.value = def.value.clone(),
            "Footprint" => f.value = def.footprint.clone(),
            "Datasheet" => f.value = def.datasheet.clone(),
            _ => {}
        }
    }
    sym
}

/// Build the design from loaded files.
pub fn elaborate(program: &Program, design_name: &str) -> (Design, Vec<Diagnostic>) {
    let mut parts_ast: HashMap<String, &ast::PartDecl> = HashMap::new();
    let mut modules: HashMap<String, &ast::ModuleDecl> = HashMap::new();
    let mut rails: HashMap<String, RailKind> = HashMap::new();
    let mut diags = Vec::new();
    let mut meta = std::collections::BTreeMap::new();
    let mut sheets: Vec<&ast::SheetDecl> = Vec::new();
    let mut top: Vec<&Stmt> = Vec::new();
    let mut uses: Vec<&ast::Use> = Vec::new();
    let std_file = program.files[0].0;

    for (fid, file) in &program.files {
        for item in &file.items {
            match item {
                Item::Part(p) => {
                    if let Some(prev) = parts_ast.get(&p.name.text)
                        && prev.span.file != std_file
                        && *fid != std_file
                    {
                        diags.push(Diagnostic::error("duplicate_part", format!("part `{}` is declared twice", p.name.text)).at(p.name.span));
                    }
                    // Later files (user code) override the standard library.
                    parts_ast.insert(p.name.text.clone(), p);
                }
                Item::Module(m) => {
                    if modules.insert(m.name.text.clone(), m).is_some() {
                        diags.push(Diagnostic::error("duplicate_module", format!("module `{}` is declared twice", m.name.text)).at(m.name.span));
                    }
                    // Ports that the body never mentions.
                    let mut mentioned = HashSet::new();
                    collect_names(&m.body, &mut mentioned);
                    for p in &m.params {
                        if !mentioned.contains(&p.text) {
                            diags.push(
                                Diagnostic::warning("unused_port", format!("port `{}` of module `{}` is never used", p.text, m.name.text))
                                    .at(p.span),
                            );
                        }
                    }
                }
                Item::Rail(r) => match RailKind::parse(&r.kind.text) {
                    Some(k) => {
                        rails.insert(r.name.text.clone(), k);
                    }
                    None => diags.push(
                        Diagnostic::error("bad_rail", format!("unknown rail kind `{}`", r.kind.text)).at(r.kind.span).hint("use ground, supply or negative"),
                    ),
                },
                Item::Sheet(s) if *fid == program.main_file => sheets.push(s),
                Item::Stmt(s) if *fid == program.main_file => top.push(s),
                Item::Meta { key, value } if *fid == program.main_file => {
                    meta.insert(key.text.clone(), value.text.clone());
                }
                Item::Use(u) => uses.push(u),
                _ => {}
            }
        }
    }
    for u in uses {
        if u.file.is_some() {
            continue;
        }
        if u.path.first().map(|p| p.text.as_str()) != Some("std") {
            diags.push(Diagnostic::error("unknown_library", format!("unknown library `{}`", u.path[0].text)).at(u.path[0].span).hint("only `std` is built in; import files with `use \"file.wx\"`"));
            continue;
        }
        for n in u.names.iter().flatten() {
            let known = parts_ast.contains_key(&n.text) || rails.contains_key(&n.text) || auto_rail(&n.text).is_some() || generated_part(&n.text).is_some();
            if !known {
                let mut all: Vec<String> = parts_ast.keys().cloned().collect();
                all.extend(rails.keys().cloned());
                let hint = suggest(&n.text, &all).map(|s| format!("did you mean `{s}`?")).unwrap_or_else(|| "see `wirx lib list`".into());
                diags.push(Diagnostic::error("unknown_import", format!("`std` has no part or rail `{}`", n.text)).at(n.span).hint(hint));
            }
        }
    }

    let top_body: Vec<Stmt> = top.into_iter().cloned().collect();
    let mut e = Elab {
        parts_ast,
        modules,
        rails,
        design: Design { name: design_name.into(), meta, ..Default::default() },
        diags,
        part_cache: HashMap::new(),
        parent: vec![],
        nodes: vec![],
        pin_nodes: vec![],
        globals: HashMap::new(),
        stack: vec![],
        explicit_refs: vec![],
    };

    let mut blocks: Vec<(String, &[Stmt])> = Vec::new();
    if !top_body.is_empty() {
        blocks.push((design_name.to_string(), &top_body));
    }
    for s in &sheets {
        blocks.push((s.name.text.clone(), &s.body));
    }
    for (name, body) in blocks {
        e.design.sheets.push(SheetIr { name: name.clone(), groups: vec![] });
        let sheet = e.design.sheets.len() - 1;
        let group = e.new_group(name.clone(), sheet, false);
        let mut scope = Scope { module: false, path: name, ports: HashMap::new(), locals: HashMap::new(), insts: HashMap::new(), group, sheet };
        e.body(&mut scope, body);
    }
    e.finish();
    // Check user parts that nothing uses yet, so library files get the same errors.
    let mut unused: Vec<&ast::PartDecl> = e.parts_ast.values().copied().filter(|p| p.span.file != std_file && !e.part_cache.contains_key(&p.name.text)).collect();
    unused.sort_by_key(|p| (p.span.file, p.span.start));
    for p in unused {
        e.convert_part(p);
    }
    (e.design, e.diags)
}

fn collect_names(body: &[Stmt], out: &mut HashSet<String>) {
    let ep = |e: &Endpoint, out: &mut HashSet<String>| {
        if let Endpoint::Net(n) = e {
            out.insert(n.text.clone());
        }
    };
    for s in body {
        match s {
            Stmt::Connect { groups, .. } => groups.iter().flatten().for_each(|e| ep(e, out)),
            Stmt::Nc { pins, .. } => pins.iter().for_each(|e| ep(e, out)),
            Stmt::PowerFlag { nets, .. } => nets.iter().for_each(|n| {
                out.insert(n.text.clone());
            }),
            Stmt::Inst { args, .. } => {
                for a in args {
                    match &a.value {
                        ArgValue::Lit(n) => {
                            out.insert(n.text.clone());
                        }
                        ArgValue::Pin(e) => ep(e, out),
                    }
                }
            }
            Stmt::Layout { .. } => {}
        }
    }
}
