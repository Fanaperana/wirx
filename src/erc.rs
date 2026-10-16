//! Electrical rules check on the elaborated design, before anything is drawn. Uses KiCad's pin
//! conflict matrix (as eleceda does), so a design that passes here also passes eleceda's checks.
//! Every finding carries a hint that is a line of wirx where possible, so an AI can apply it.

use crate::diag::Diagnostic;
use crate::ir::Design;
use crate::symbol::PinType;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Err,
}
use Level::*;

/// KiCad's default ERC pin matrix; order follows `PinType::ALL`.
const MATRIX: [[Level; 12]; 12] = [
    //  I    O    Bi   3S   Pas  Free UnS  PwI  PwO  OC   OE   NC
    [Ok, Ok, Ok, Ok, Ok, Ok, Warn, Ok, Ok, Ok, Ok, Err],                   // input
    [Ok, Err, Ok, Warn, Ok, Ok, Warn, Ok, Err, Err, Err, Err],             // output
    [Ok, Ok, Ok, Ok, Ok, Ok, Warn, Ok, Warn, Ok, Warn, Err],               // bidirectional
    [Ok, Warn, Ok, Ok, Ok, Ok, Warn, Warn, Err, Warn, Warn, Err],          // tri_state
    [Ok, Ok, Ok, Ok, Ok, Ok, Warn, Ok, Ok, Ok, Ok, Err],                   // passive
    [Ok, Ok, Ok, Ok, Ok, Ok, Ok, Ok, Ok, Ok, Ok, Err],                     // free
    [Warn, Warn, Warn, Warn, Warn, Ok, Warn, Warn, Warn, Warn, Warn, Err], // unspecified
    [Ok, Ok, Ok, Warn, Ok, Ok, Warn, Ok, Ok, Ok, Ok, Err],                 // power_in
    [Ok, Err, Warn, Err, Ok, Ok, Warn, Ok, Err, Err, Err, Err],            // power_out
    [Ok, Err, Ok, Warn, Ok, Ok, Warn, Ok, Err, Ok, Ok, Err],               // open_collector
    [Ok, Err, Warn, Warn, Ok, Ok, Warn, Ok, Err, Ok, Ok, Err],             // open_emitter
    [Err, Err, Err, Err, Err, Err, Err, Err, Err, Err, Err, Err],          // no_connect
];

const DRIVERS: [PinType; 7] = [
    PinType::Output,
    PinType::PowerOut,
    PinType::Bidirectional,
    PinType::TriState,
    PinType::Passive,
    PinType::OpenCollector,
    PinType::OpenEmitter,
];

/// How a script refers to this pin: `x.VIN`, `x[1]` or `x["~RESET"]`.
fn script_pin(var: &str, name: &str, number: &str) -> String {
    let ident = !name.is_empty() && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ident {
        format!("{var}.{name}")
    } else if number.chars().all(|c| c.is_ascii_digit()) {
        format!("{var}[{number}]")
    } else {
        format!("{var}[\"{number}\"]")
    }
}

/// How a script refers to this net: a built-in rail constant, `supply("…")` or `net("…")`.
fn script_net(name: &str, rail: bool) -> String {
    if let Some(r) = crate::stdlib::RAILS.iter().find(|r| r.1 == name) {
        return r.0.to_string();
    }
    if rail { format!("supply(\"{name}\")") } else { format!("net(\"{name}\")") }
}

pub fn check(d: &Design) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    // Pins: unconnected, or `nc` but connected.
    for (i, inst) in d.instances.iter().enumerate() {
        let part = &d.parts[inst.part];
        for (k, pin) in part.pins.iter().enumerate() {
            let net = inst.pin_net[k].map(|n| &d.nets[n]);
            let others = net.map(|n| n.pins.len() - 1).unwrap_or(0);
            if inst.nc[k] {
                if others > 0 || net.is_some_and(|n| n.is_rail()) {
                    out.push(
                        Diagnostic::error("nc_connected", format!("{} is marked `nc` but connected to {}", d.pin_label(i, k), net.unwrap().name))
                            .at(inst.span),
                    );
                }
                continue;
            }
            if pin.hidden || pin.ptype == PinType::NoConnect || part.open_pins_ok {
                continue;
            }
            let rail = net.is_some_and(|n| n.is_rail());
            if others == 0 && !rail {
                let named = net.filter(|n| n.named).map(|n| n.name.clone());
                let msg = match &named {
                    Some(n) => format!("{} is the only pin on net `{n}`", d.pin_label(i, k)),
                    None => format!("{} is not connected", d.pin_label(i, k)),
                };
                out.push(
                    Diagnostic::error("pin_not_connected", msg)
                        .at(inst.span)
                        .hint(format!("connect it, or mark it unused: `{} -> NC` (where `x` holds {})", script_pin("x", &pin.name, &pin.number), inst.reference)),
                );
            }
        }
    }

    for net in &d.nets {
        // Pin type conflicts: report the worst pair once per net.
        let mut worst: Option<(Level, usize, usize)> = None;
        for a in 0..net.pins.len() {
            for b in a + 1..net.pins.len() {
                let (ta, tb) = (d.pin(net.pins[a].0, net.pins[a].1).ptype, d.pin(net.pins[b].0, net.pins[b].1).ptype);
                if ta == PinType::Unspecified || tb == PinType::Unspecified {
                    continue;
                }
                let level = MATRIX[ta.index()][tb.index()];
                if level != Ok && worst.is_none_or(|w| level == Err && w.0 == Warn) {
                    worst = Some((level, a, b));
                }
            }
        }
        if let Some((level, a, b)) = worst {
            let (pa, pb) = (net.pins[a], net.pins[b]);
            let msg = format!(
                "net `{}`: {} ({}) conflicts with {} ({})",
                net.name,
                d.pin_label(pa.0, pa.1),
                d.pin(pa.0, pa.1).ptype.name(),
                d.pin_label(pb.0, pb.1),
                d.pin(pb.0, pb.1).ptype.name()
            );
            let diag = if level == Err { Diagnostic::error("pin_conflict", msg) } else { Diagnostic::warning("pin_conflict", msg) };
            out.push(diag.at(d.instances[pb.0].span));
        }

        let types: Vec<PinType> = net.pins.iter().map(|&(i, k)| d.pin(i, k).ptype).collect();
        // Power inputs with nothing that supplies them.
        let powered = types.contains(&PinType::PowerIn);
        if powered && !types.contains(&PinType::PowerOut) && !net.flagged {
            let users: Vec<String> = net.pins.iter().filter(|&&(i, k)| d.pin(i, k).ptype == PinType::PowerIn).take(4).map(|&(i, k)| d.pin_label(i, k)).collect();
            out.push(
                Diagnostic::warning("power_not_driven", format!("net `{}` powers {} but nothing drives it", net.name, users.join(", ")))
                    .at_opt(net.span)
                    .hint(format!("if power comes from a connector or off-board, mark it driven: `{}.driven();`", script_net(&net.name, net.is_rail()))),
            );
        }
        // Inputs with no driver on an otherwise connected net.
        if types.contains(&PinType::Input) && net.pins.len() > 1 && !types.iter().any(|t| DRIVERS.contains(t)) && !net.named {
            out.push(Diagnostic::warning("input_not_driven", format!("net `{}` has inputs but no driver", net.name)).at_opt(net.span));
        }
    }

    // Manufacturing readiness.
    for inst in &d.instances {
        if inst.footprint.trim().is_empty() {
            out.push(
                Diagnostic::warning("missing_footprint", format!("{} ({}) has no footprint", inst.reference, d.parts[inst.part].name))
                    .at(inst.span)
                    .hint("pass one: `X(\"value\", \"Lib:Footprint\")`, or set `footprint:` in the part"),
            );
        }
        if inst.value.trim().is_empty() {
            out.push(Diagnostic::warning("missing_value", format!("{} has no value", inst.reference)).at(inst.span));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn matrix_is_symmetric() {
        for a in 0..12 {
            for b in 0..12 {
                assert!(super::MATRIX[a][b] == super::MATRIX[b][a], "{a},{b}");
            }
        }
    }
}
