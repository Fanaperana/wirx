//! Building the design: a front-end-agnostic builder (nets as union-find nodes, instances,
//! groups and sheets) plus the naming rules shared by front ends. The Rhai front end in
//! `crate::script` drives it.

use crate::diag::{Diagnostic, Span};
use crate::ir::*;
use crate::symbol::boxgen::{box_symbol, BoxPin};
use crate::symbol::templates::{self, connector, connector_dual, from_template};
use crate::symbol::{PinType, RailKind, SymbolDef};
use std::collections::{HashMap, HashSet};

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
pub fn footprint_alias(symbol: &str, size: &str) -> Option<String> {
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
pub fn generated_part(name: &str) -> Option<PartDef> {
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
        custom: None,
    })
}

/// The symbol drawn for a part.
pub fn build_symbol(def: &PartDef) -> SymbolDef {
    let id = format!("wirx:{}", def.name);
    let pins: Vec<(String, String, PinType, bool)> = def.pins.iter().map(|p| (p.number.clone(), p.name.clone(), p.ptype, p.hidden)).collect();
    // A part with its own symbol (defined in wirx or from a library) keeps that drawing.
    if let Some(custom) = &def.custom {
        let mut sym = (**custom).clone();
        sym.id = id;
        sym.name = def.name.clone();
        sym.ref_prefix = def.prefix.clone();
        sym.unit_count = 1;
        sym.fields = templates::std_fields(&def.prefix, &def.name);
        return finish_symbol(def, sym);
    }
    let sym = match def.symbol.as_str() {
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
    finish_symbol(def, sym)
}

/// Description, flags and field values every part symbol carries.
fn finish_symbol(def: &PartDef, mut sym: SymbolDef) -> SymbolDef {
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

#[derive(Default, Clone)]
struct NodeInfo {
    /// (priority, name, span): rail 4, named net 3.
    names: Vec<(u8, String, Option<Span>)>,
    flagged: bool,
}

/// Collects parts, instances and connections, then names everything in [`Builder::finish`].
pub struct Builder {
    pub design: Design,
    pub diags: Vec<Diagnostic>,
    parent: Vec<usize>,
    nodes: Vec<NodeInfo>,
    pin_nodes: Vec<Vec<usize>>,
    rails: HashMap<String, RailKind>,
    named: HashMap<String, usize>,
    /// Part name -> (part index, definition fingerprint).
    parts: HashMap<String, (usize, String)>,
    explicit_refs: Vec<Option<(String, Option<Span>)>>,
}

/// Everything about a new instance except its pins.
pub struct NewInstance {
    pub part: usize,
    pub value: String,
    pub footprint: String,
    pub fields: Vec<(String, String)>,
    pub dnp: bool,
    pub reference: Option<String>,
    pub group: usize,
    pub sheet: usize,
    pub span: Option<Span>,
}

impl Builder {
    pub fn new(name: &str) -> Builder {
        Builder {
            design: Design { name: name.into(), ..Default::default() },
            diags: vec![],
            parent: vec![],
            nodes: vec![],
            pin_nodes: vec![],
            rails: HashMap::new(),
            named: HashMap::new(),
            parts: HashMap::new(),
            explicit_refs: vec![],
        }
    }

    pub fn new_node(&mut self) -> usize {
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

    pub fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
            let moved = std::mem::take(&mut self.nodes[ra]);
            self.nodes[rb].names.extend(moved.names);
            self.nodes[rb].flagged |= moved.flagged;
        }
    }

    pub fn rail_kind(&self, name: &str) -> Option<RailKind> {
        self.rails.get(name).copied().or_else(|| auto_rail(name))
    }

    /// The net with this name (a rail if the name is one); the same name is always the same net.
    pub fn net(&mut self, name: &str, span: Option<Span>) -> usize {
        if let Some(&n) = self.named.get(name) {
            return n;
        }
        let prio = if self.rail_kind(name).is_some() { 4 } else { 3 };
        let n = self.new_node();
        self.nodes[n].names.push((prio, name.to_string(), span));
        self.named.insert(name.to_string(), n);
        n
    }

    /// Declare a rail. Declaring the same name again with the same kind is fine.
    pub fn rail(&mut self, name: &str, kind: RailKind, span: Option<Span>) -> Result<usize, String> {
        match self.rail_kind(name) {
            Some(k) if k != kind => return Err(format!("`{name}` is already a {} rail", rail_word(k))),
            None if self.named.contains_key(name) => return Err(format!("`{name}` is already used as a signal net")),
            _ => {}
        }
        self.rails.insert(name.to_string(), kind);
        Ok(self.net(name, span))
    }

    pub fn flag(&mut self, node: usize) {
        let r = self.find(node);
        self.nodes[r].flagged = true;
    }

    /// Register a part definition; the same name must always mean the same part.
    pub fn add_part(&mut self, def: PartDef) -> Result<usize, String> {
        let mut key_def = def.clone();
        key_def.span = None;
        let fp = format!("{key_def:?}");
        if let Some((i, prev)) = self.parts.get(&def.name) {
            if *prev == fp {
                return Ok(*i);
            }
            return Err(format!("part `{}` is defined twice with different contents", def.name));
        }
        let sym = build_symbol(&def);
        let name = def.name.clone();
        self.design.parts.push(def);
        self.design.symbols.push(sym);
        let i = self.design.parts.len() - 1;
        self.parts.insert(name, (i, fp));
        Ok(i)
    }

    pub fn add_instance(&mut self, n: NewInstance) -> usize {
        let npins = self.design.parts[n.part].pins.len();
        let id = self.design.instances.len();
        self.design.instances.push(Instance {
            path: String::new(),
            local: String::new(),
            part: n.part,
            reference: String::new(),
            value: n.value,
            footprint: n.footprint,
            fields: n.fields,
            dnp: n.dnp,
            group: n.group,
            sheet: n.sheet,
            span: n.span.unwrap_or_default(),
            pin_net: vec![None; npins],
            nc: vec![false; npins],
        });
        self.explicit_refs.push(n.reference.map(|r| (r, n.span)));
        let nodes = (0..npins).map(|_| self.new_node()).collect();
        self.pin_nodes.push(nodes);
        self.design.groups[n.group].instances.push(id);
        id
    }

    pub fn pin_node(&self, inst: usize, pin: usize) -> usize {
        self.pin_nodes[inst][pin]
    }

    pub fn set_nc(&mut self, inst: usize, pin: usize) {
        self.design.instances[inst].nc[pin] = true;
    }

    pub fn add_sheet(&mut self, name: &str) -> usize {
        if let Some(i) = self.design.sheets.iter().position(|s| s.name == name) {
            return i;
        }
        self.design.sheets.push(SheetIr { name: name.into(), groups: vec![] });
        self.design.sheets.len() - 1
    }

    pub fn add_group(&mut self, name: String, sheet: usize, framed: bool) -> usize {
        self.design.groups.push(Group { name, sheet, instances: vec![], near: vec![], flow: 0, framed });
        self.design.sheets[sheet].groups.push(self.design.groups.len() - 1);
        self.design.groups.len() - 1
    }

    /// Name nets, assign references and return the finished design.
    pub fn finish(mut self) -> (Design, Vec<Diagnostic>) {
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
            }
        }
        let mut roots: Vec<(usize, usize)> = by_root.into_iter().collect();
        roots.sort();
        for (root, n) in roots {
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

        // References: explicit first, then numbered in creation order per prefix.
        let mut used: HashSet<String> = HashSet::new();
        for (i, r) in self.explicit_refs.clone().into_iter().enumerate() {
            if let Some((r, span)) = r {
                if !used.insert(r.clone()) {
                    self.diags.push(Diagnostic::error("duplicate_reference", format!("reference `{r}` is used twice")).at_opt(span));
                }
                self.design.instances[i].reference = r;
            }
        }
        let mut next: HashMap<String, u32> = HashMap::new();
        for i in 0..self.design.instances.len() {
            if self.design.instances[i].reference.is_empty() {
                let prefix = self.design.parts[self.design.instances[i].part].prefix.clone();
                let n = next.entry(prefix.clone()).or_insert(1);
                while used.contains(&format!("{prefix}{n}")) {
                    *n += 1;
                }
                let r = format!("{prefix}{n}");
                used.insert(r.clone());
                self.design.instances[i].reference = r;
            }
            let sheet = self.design.sheets[self.design.instances[i].sheet].name.clone();
            let inst = &mut self.design.instances[i];
            inst.local = inst.reference.clone();
            inst.path = format!("{sheet}/{}", inst.reference);
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
        // Groups that ended up empty are dropped from their sheets.
        let groups = &self.design.groups;
        for s in &mut self.design.sheets {
            s.groups.retain(|&g| !groups[g].instances.is_empty());
        }
        (self.design, self.diags)
    }
}

fn rail_word(k: RailKind) -> &'static str {
    match k {
        RailKind::Ground => "ground",
        RailKind::Supply => "supply",
        RailKind::Negative => "negative",
    }
}
