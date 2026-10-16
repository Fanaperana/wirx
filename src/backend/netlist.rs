//! Netlist exports for PCB tools (KiCad S-expression) and for scripts/AI (JSON).

use crate::ir::Design;
use crate::sema::natural_cmp;
use serde_json::json;

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// KiCad netlist (`.net`, version E), readable by Pcbnew and most PCB tools.
pub fn kicad(d: &Design, source: &str) -> String {
    let mut out = String::new();
    out.push_str("(export (version \"E\")\n");
    out.push_str(&format!("  (design (source {}) (tool \"wirx {}\"))\n", q(source), env!("CARGO_PKG_VERSION")));
    out.push_str("  (components\n");
    let mut insts: Vec<usize> = (0..d.instances.len()).collect();
    insts.sort_by(|&a, &b| natural_cmp(&d.instances[a].reference, &d.instances[b].reference));
    for i in &insts {
        let inst = &d.instances[*i];
        let part = &d.parts[inst.part];
        out.push_str(&format!(
            "    (comp (ref {}) (value {}) (footprint {})\n      (libsource (lib \"wirx\") (part {}) (description {}))\n",
            q(&inst.reference),
            q(&inst.value),
            q(&inst.footprint),
            q(&part.name),
            q(&part.description)
        ));
        for (k, v) in &inst.fields {
            out.push_str(&format!("      (property (name {}) (value {}))\n", q(k), q(v)));
        }
        if inst.dnp {
            out.push_str("      (property (name \"dnp\"))\n");
        }
        out.push_str(&format!("      (sheetpath (names {}) (tstamps \"/\")))\n", q(&format!("/{}/", d.sheets[inst.sheet].name))));
    }
    out.push_str("  )\n  (nets\n");
    for (code, net) in d.nets.iter().enumerate() {
        out.push_str(&format!("    (net (code \"{}\") (name {})\n", code + 1, q(&net.name)));
        let mut pins = net.pins.clone();
        pins.sort_by(|a, b| natural_cmp(&d.instances[a.0].reference, &d.instances[b.0].reference).then(a.1.cmp(&b.1)));
        for (i, k) in pins {
            let p = d.pin(i, k);
            out.push_str(&format!(
                "      (node (ref {}) (pin {}) (pinfunction {}) (pintype {}))\n",
                q(&d.instances[i].reference),
                q(&p.number),
                q(&p.name),
                q(p.ptype.name())
            ));
        }
        out.push_str("    )\n");
    }
    out.push_str("  )\n)\n");
    out
}

/// Compact JSON netlist: components and nets with pin names, for review tools and AI.
pub fn json(d: &Design) -> serde_json::Value {
    let components: Vec<_> = d
        .instances
        .iter()
        .map(|i| {
            json!({
                "ref": i.reference,
                "part": d.parts[i.part].name,
                "value": i.value,
                "footprint": i.footprint,
                "path": i.path,
                "sheet": d.sheets[i.sheet].name,
                "group": d.groups[i.group].name,
            })
        })
        .collect();
    let nets: Vec<_> = d
        .nets
        .iter()
        .map(|n| {
            json!({
                "name": n.name,
                "rail": n.is_rail(),
                "pins": n.pins.iter().map(|&(i, k)| {
                    let p = d.pin(i, k);
                    json!({ "ref": d.instances[i].reference, "pin": p.number, "name": p.name, "type": p.ptype.name() })
                }).collect::<Vec<_>>()
            })
        })
        .collect();
    json!({ "design": d.name, "components": components, "nets": nets })
}
