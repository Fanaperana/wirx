//! End-to-end tests: examples build cleanly and deterministically, scripts build the intended
//! design, and mistakes give located errors with useful hints.

use std::path::Path;
use wirx::diag::{has_errors, Severity};

fn check(src: &str) -> wirx::Compiled {
    wirx::check_source("test.wx", src, Path::new("."))
}

fn codes(c: &wirx::Compiled, sev: Severity) -> Vec<&'static str> {
    c.diags.iter().filter(|d| d.severity == sev).map(|d| d.code).collect()
}

fn messages(c: &wirx::Compiled) -> Vec<String> {
    c.diags.iter().map(|d| c.sources.render(d)).collect()
}

fn net_pins(d: &wirx::ir::Design, name: &str) -> Vec<String> {
    let net = d.nets.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("no net {name}"));
    let mut v: Vec<String> = net.pins.iter().map(|&(i, k)| format!("{}.{}", d.instances[i].reference, d.pin(i, k).number)).collect();
    v.sort();
    v
}

#[test]
fn examples_build_without_errors_and_deterministically() {
    for name in ["ldo", "blinker", "mcu"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(format!("{name}.wx"));
        let c = wirx::check_file(&path).unwrap();
        assert!(c.diags.iter().all(|d| d.severity == Severity::Info), "{name}: {:#?}", messages(&c));
        let design = c.design.unwrap();
        let (a, layout_diags) = wirx::build(&design).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
        assert!(layout_diags.is_empty(), "{name}: {:?}", layout_diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        let (b, _) = wirx::build(&design).unwrap();
        assert_eq!(a, b, "{name}: output must be deterministic");
        assert_eq!(a["format"], "eleceda-schematic");
        for sheet in a["sheets"].as_array().unwrap() {
            for it in sheet["items"].as_array().unwrap() {
                if it["kind"] == "symbol" {
                    assert!(a["symbols"].get(it["libId"].as_str().unwrap()).is_some());
                }
            }
        }
    }
}

#[test]
fn chains_join_nets_and_parts_pass_through() {
    let c = check(
        r#"
        let u = NE555();
        P5V -> [u.VCC, u.R];
        u.GND -> GND;
        u.Q -> R("330") -> LED("red") -> GND;      // R in, LED anode in, cathode out
        nc([u.TR, u.THR, u.CV, u.DIS]);
        P5V.driven(); GND.driven();
        "#,
    );
    assert!(!has_errors(&c.diags), "{:#?}", messages(&c));
    let d = c.design.unwrap();
    assert_eq!(net_pins(&d, "+5V"), ["U1.4", "U1.8"]);
    assert_eq!(net_pins(&d, "U1_Q"), ["R1.1", "U1.3"]);
    // LED pins are K=1, A=2: the resistor reaches the anode, the cathode goes to ground.
    assert_eq!(net_pins(&d, "GND"), ["D1.1", "U1.1"]);
    assert!(d.nets.iter().any(|n| n.pins.len() == 2 && n.pins.iter().any(|&(i, k)| d.instances[i].reference == "D1" && d.pin(i, k).name == "A")));
}

#[test]
fn functions_become_groups_and_loops_work() {
    let c = check(
        r#"
        fn indicator(drive) { drive -> R("330") -> LED("red") -> GND; }
        fn ATtiny() { part(#{ name: "ATtiny", pins: [[1, "PB0"], [2, "PB1"], [3, "PB2"], [[4, 5], "GND", power_in], [6, "VCC", power_in]] }) }
        sheet("Main", || {
            let u = ATtiny();
            u.VCC -> P5V.driven();
            u.GND -> GND.driven();
            for i in 0..3 { indicator(u.pin(`PB${i}`)); }
        });
        "#,
    );
    assert!(!has_errors(&c.diags), "{:#?}", messages(&c));
    let d = c.design.unwrap();
    assert_eq!(d.instances.len(), 7);
    let framed: Vec<&str> = d.groups.iter().filter(|g| g.framed && !g.instances.is_empty()).map(|g| g.name.as_str()).collect();
    // One call site in a loop is one group; the part constructor is not a group.
    assert_eq!(framed, ["indicator"]);
    let (esch, _) = wirx::build(&d).unwrap();
    assert_eq!(esch["sheets"][0]["name"], "Main");
}

#[test]
fn separate_call_sites_are_separate_groups() {
    let c = check(
        r#"
        fn divider(top, tap) { top -> R("10k") -> tap -> R("10k") -> GND; }
        divider(P5V, net("A"));
        divider(P5V, net("B"));
        let j = Conn(2);
        j[1] -> net("A");
        j[2] -> net("B");
        "#,
    );
    let d = c.design.unwrap();
    let mut framed: Vec<&str> = d.groups.iter().filter(|g| g.framed).map(|g| g.name.as_str()).collect();
    framed.sort();
    assert_eq!(framed, ["divider", "divider 2"]);
    assert_eq!(net_pins(&d, "A"), ["J1.1", "R1.2", "R2.1"]);
}

#[test]
fn user_names_win_over_built_in_constants() {
    let c = check(
        r#"
        fn f(left) { left -> R("1k") -> GND; }
        let input = net("IN");
        sheet("S", || { f(input); input -> R("2k") -> GND; });
        "#,
    );
    let d = c.design.unwrap();
    assert_eq!(net_pins(&d, "IN"), ["R1.1", "R2.1"]);
}

#[test]
fn unknown_pin_suggests_the_right_one() {
    let c = check("let u = AMS1117_3V3();\nP5V -> u.VI;");
    let d = c.diags.iter().find(|d| d.code == "unknown_pin").expect("unknown_pin");
    assert!(d.hint.as_ref().unwrap().contains(".VIN"), "{:?}", d.hint);
    let s = d.span.unwrap();
    assert_eq!(c.sources.line_col(s.file, s.start), (2, 10));
}

#[test]
fn errors_inside_functions_point_at_the_line() {
    let c = check("fn f(x) {\n    x -> R(\"1k\") -> GND;\n}\nf(\"oops\");");
    let d = c.diags.iter().find(|d| d.code == "string_as_net").unwrap();
    assert_eq!(c.sources.line_col(d.span.unwrap().file, d.span.unwrap().start).0, 2);
    assert!(d.hint.as_ref().unwrap().contains("net(\"oops\")"));
}

#[test]
fn unconnected_pin_is_an_error() {
    let c = check("let r = R(\"1k\");\nr[1] -> GND;");
    let d = c.diags.iter().find(|d| d.code == "pin_not_connected").unwrap();
    assert!(d.message.contains("R1.2"), "{}", d.message);
}

#[test]
fn nc_pins_are_fine_but_not_when_connected() {
    let ok = check("let r = R(\"1k\");\nr[1] -> GND;\nr[2] -> NC;");
    assert!(!has_errors(&ok.diags), "{:?}", codes(&ok, Severity::Error));
    let bad = check("let r = R(\"1k\");\nr[1] -> GND;\nr[2] -> GND;\nnc(r[2]);");
    assert!(codes(&bad, Severity::Error).contains(&"nc_connected"));
}

#[test]
fn shorted_rails_are_reported() {
    let c = check("let r = R(\"1k\");\nP5V -> GND -> r[1];\nr[2] -> GND;");
    assert!(codes(&c, Severity::Error).contains(&"rails_shorted"));
}

#[test]
fn two_outputs_conflict() {
    let c = check(
        r#"
        fn Buf() { part(#{ name: "Buf", pins: [[1, "A", input], [2, "Y", output], [3, "VCC", power_in], [4, "GND", power_in]] }) }
        let a = Buf();
        let b = Buf();
        a.Y -> b.Y;
        [a.A, b.A] -> R("1k") -> GND;
        P5V.driven() -> [a.VCC, b.VCC];
        GND.driven() -> [a.GND, b.GND];
        "#,
    );
    assert!(codes(&c, Severity::Error).contains(&"pin_conflict"), "{:#?}", messages(&c));
}

#[test]
fn power_needs_a_driver_or_driven() {
    let src = "let u = NE555();\nP5V -> [u.VCC, u.R];\nu.GND -> GND;\nnc([u.TR, u.THR, u.CV, u.Q, u.DIS]);";
    let c = check(src);
    let d = c.diags.iter().find(|d| d.code == "power_not_driven").expect("power_not_driven");
    assert!(d.hint.as_ref().unwrap().contains("P5V.driven()"), "{:?}", d.hint);
    let fixed = check(&format!("{src}\nP5V.driven();\nGND.driven();"));
    assert!(!fixed.diags.iter().any(|d| d.code == "power_not_driven"));
}

#[test]
fn references_and_values() {
    let c = check(
        r#"
        let a = R(4700);
        let b = R("1k", "0402", #{ ref: "R1", mpn: "X" });
        let c = C(0.0000001, "0402");
        let r = R(e24(4600.0));
        [a[1], b[1], c[1], r[1]] -> GND;
        [a[2], b[2], c[2], r[2]] -> net("X");
        "#,
    );
    let d = c.design.unwrap();
    assert_eq!(d.instances[0].reference, "R2");
    assert_eq!(d.instances[0].value, "4k7");
    assert_eq!(d.instances[1].reference, "R1");
    assert_eq!(d.instances[1].footprint, "Resistor_SMD:R_0402_1005Metric");
    assert!(d.instances[1].fields.contains(&("MPN".into(), "X".into())));
    assert_eq!(d.instances[2].value, "100nF");
    assert_eq!(d.instances[3].value, "4k7");
}

#[test]
fn custom_part_mistakes_are_explained() {
    let c = check("fn Q() { part(#{ name: \"Q\", symbol: \"npn\", pins: [[1, \"B\"], [2, \"C\"]] }) }\nlet q = Q();");
    let d = c.diags.iter().find(|d| d.code == "template_pin_count").unwrap();
    assert!(d.message.contains("B, C, E"), "{}", d.message);
    let c = check("let p = part(#{ name: \"X\", pins: [[1, \"A\", inptu]] });");
    assert!(codes(&c, Severity::Error).contains(&"unknown_variable"), "{:#?}", messages(&c));
}

#[test]
fn multi_pin_part_cannot_sit_inline() {
    let c = check("let u = NE555();\nP5V -> u -> GND;");
    assert!(codes(&c, Severity::Error).contains(&"not_inline"));
}

/// Layout optimisations may never replace a better default result with a worse one: for every
/// group of every example, the optimised layout has no more collisions, unrouted pins or four-way
/// points, and ranks no worse on the shared quality order (`GroupStats::rank`, displacement aside).
#[test]
fn optimisations_never_make_a_group_worse() {
    use wirx::layout::Options;
    for name in ["ldo", "blinker", "mcu"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(format!("{name}.wx"));
        let design = wirx::check_file(&path).unwrap().design.unwrap();
        let (plain, _) = wirx::draw_with(&design, &Options::none()).unwrap();
        let (best, _) = wirx::draw_with(&design, &Options::default()).unwrap();
        for (a, b) in plain.stats.iter().zip(&best.stats) {
            assert_eq!(a.group, b.group);
            assert!(b.collisions <= a.collisions && b.unrouted <= a.unrouted && b.four_way <= a.four_way, "{name}/{}: {a:?} -> {b:?}", a.group);
            let rank = |s: &wirx::layout::GroupStats| { let r = s.rank(); (r.0, r.1, r.2, r.3, r.4, r.5) };
            assert!(rank(b) <= rank(a), "{name}/{}: {a:?} -> {b:?}", a.group);
        }
    }
}

/// The fixtures: the USB, LDO and blinker groups stay crossing-free, and the MCU group (which
/// had one crossing with the default placement) has none.
#[test]
fn layout_fixtures() {
    let stats = |name: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(format!("{name}.wx"));
        let design = wirx::check_file(&path).unwrap().design.unwrap();
        wirx::draw(&design).unwrap().0.stats
    };
    for (name, group) in [("ldo", "Power"), ("ldo", "ldo"), ("blinker", "astable"), ("mcu", "mcu"), ("mcu", "Power")] {
        let s = stats(name);
        let g = s.iter().find(|g| g.group == group).unwrap();
        assert_eq!((g.crossings, g.collisions, g.unrouted, g.four_way), (0, 0, 0, 0), "{name}/{group}: {g:?}");
    }
}

/// The RP2350B board (examples/rp2350b.wx) passes the electrical checks, builds, verifies and
/// is deterministic. Every pin is wired: the core regulator on the chip's crowded top edge too.
#[test]
fn rp2350b_board_builds() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/rp2350b.wx");
    let c = wirx::check_file(&path).unwrap();
    assert!(c.diags.iter().all(|d| d.severity == Severity::Info), "{:#?}", messages(&c));
    let design = c.design.unwrap();
    assert_eq!((design.instances.len(), design.nets.len(), design.sheets.len()), (46, 79, 3));
    let (a, layout_diags) = wirx::build(&design).unwrap();
    assert!(layout_diags.is_empty(), "{:?}", layout_diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    let (b, _) = wirx::build(&design).unwrap();
    assert_eq!(a, b);
}

/// A symbol laid out in wirx: pins where the author puts them, gaps between groups, stacked pins
/// drawn once; the part's pins come from the symbol.
#[test]
fn symbols_defined_in_wirx() {
    let c = check(
        r#"
        fn Chip() {
            part(#{ name: "Chip", symbol: #{
                left: [[1, "IN", input], gap, [2, "EN", input]],
                right: [[3, "OUT", output]],
                top: [[[4, 5, 6], "VDD", power_in]],
                bottom: [[7, "GND", power_in]],
            } })
        }
        let u = Chip();
        P3V3.driven() -> u.VDD;
        GND.driven() -> u.GND;
        u.IN -> R("1k") -> GND;
        u.EN -> R("10k") -> P3V3;
        u.OUT -> NC;
        "#,
    );
    assert!(!has_errors(&c.diags), "{:#?}", messages(&c));
    let d = c.design.unwrap();
    let sym = &d.symbols[d.instances[0].part];
    let pin = |n: &str| sym.pins.iter().find(|p| p.number == n).unwrap();
    // The gap leaves one empty place between IN and EN; VDD's three numbers share one spot.
    assert_eq!(pin("2").at.y - pin("1").at.y, 200);
    assert_eq!((pin("4").at, pin("5").at, pin("6").at), (pin("4").at, pin("4").at, pin("4").at));
    assert_eq!((pin("4").hidden, pin("5").hidden), (None, Some(true)));
    wirx::build(&d).unwrap();
}

/// Symbols written by wirx come back through `library:` (and through KiCad files), so a symbol
/// defined once resolves anywhere.
#[test]
fn symbols_round_trip_through_libraries() {
    let dir = std::env::temp_dir().join(format!("wirx-lib-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = r#"fn Chip() { part(#{ name: "Chip", symbol: #{ left: [[1, "A", input], [2, "B", input]], right: [[3, "Y", output]], top: [[[4, 5], "VDD", power_in]], bottom: [[6, "GND", power_in]] } }) }
        let u = Chip(); P3V3.driven() -> u.VDD; GND.driven() -> u.GND; [u.A, u.B] -> GND; u.Y -> NC;"#;
    let d = check(src).design.unwrap();
    let chip = d.symbols.iter().find(|s| s.name == "Chip").unwrap().clone();
    std::fs::write(dir.join("chips.elib"), wirx::symbol::library::write_elib("wirx", &[chip.clone()])).unwrap();
    std::fs::write(dir.join("chips.kicad_sym"), wirx::symbol::library::write_kicad_sym(&[chip.clone()])).unwrap();
    for lib in ["chips.elib", "chips.kicad_sym"] {
        let text = format!(
            r#"fn Chip() {{ part(#{{ name: "Chip", library: "{lib}", symbol: "Chip" }}) }}
            let u = Chip(); P3V3.driven() -> u.VDD; GND.driven() -> u.GND; [u.A, u.B] -> GND; u.Y -> NC;"#
        );
        let c = wirx::check_source("t.wx", &text, &dir);
        assert!(!has_errors(&c.diags), "{lib}: {:#?}", messages(&c));
        let back = c.design.unwrap();
        let sym = &back.symbols[back.instances[0].part];
        let pins = |s: &wirx::symbol::SymbolDef| s.pins.iter().map(|p| (p.number.clone(), p.at, p.dir, p.ptype)).collect::<Vec<_>>();
        assert_eq!(pins(sym), pins(&chip), "{lib}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Library pin names keep working in scripts: `GPIO40/ADC0` answers to GPIO40 and ADC0,
/// `~{CS}` to CS.
#[test]
fn library_pin_names_have_short_forms() {
    let c = check(
        r#"fn F() { part(#{ name: "F", symbol: #{ left: [[1, "~{CS}", input], [2, "GPIO40/ADC0", bidirectional]], bottom: [[3, "GND", power_in]] } }) }
        let u = F(); u.CS -> R("10k") -> GND.driven(); u.ADC0 -> net("A"); u.GPIO40 -> R("1k") -> GND; u.GND -> GND;"#,
    );
    assert!(!has_errors(&c.diags), "{:#?}", messages(&c));
}
