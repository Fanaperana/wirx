//! End-to-end tests: examples build cleanly and deterministically, and mistakes give useful errors.

use std::path::Path;
use wirx::diag::{has_errors, Severity};

fn check(src: &str) -> wirx::Compiled {
    wirx::check_source("test.wx", src, Path::new("."))
}

fn codes(c: &wirx::Compiled, sev: Severity) -> Vec<&'static str> {
    c.diags.iter().filter(|d| d.severity == sev).map(|d| d.code).collect()
}

#[test]
fn examples_build_without_errors_and_deterministically() {
    for name in ["ldo", "blinker", "mcu"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(format!("{name}.wx"));
        let c = wirx::check_file(&path).unwrap();
        let msgs: Vec<String> = c.diags.iter().map(|d| c.sources.render(d)).collect();
        assert!(c.diags.iter().all(|d| d.severity == Severity::Info), "{name}: {msgs:#?}");
        let design = c.design.unwrap();
        let (a, layout_diags) = wirx::build(&design).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
        assert!(layout_diags.is_empty(), "{name}: {:?}", layout_diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        let (b, _) = wirx::build(&design).unwrap();
        assert_eq!(a, b, "{name}: output must be deterministic");
        assert_eq!(a["format"], "eleceda-schematic");
        // Every symbol used is embedded.
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
fn unknown_pin_suggests_the_right_one() {
    let c = check("sheet \"A\" { u = AMS1117_3V3\n u.VI -- X }");
    let d = c.diags.iter().find(|d| d.code == "unknown_pin").expect("unknown_pin");
    assert!(d.hint.as_ref().unwrap().contains("u.VIN"), "{:?}", d.hint);
}

#[test]
fn unknown_part_suggests_and_points_at_it() {
    let c = check("sheet \"A\" { y = Crystl(16MHz) }");
    let d = c.diags.iter().find(|d| d.code == "unknown_type").unwrap();
    assert!(d.hint.as_ref().unwrap().contains("`Crystal`"), "{:?}", d.hint);
    assert_eq!(c.sources.line_col(d.span.unwrap().file, d.span.unwrap().start), (1, 17));
}

#[test]
fn unconnected_pin_is_an_error_with_a_fix() {
    let c = check("sheet \"A\" { r = R(1k)\n r.1 -- GND }");
    let d = c.diags.iter().find(|d| d.code == "pin_not_connected").unwrap();
    assert!(d.message.contains("R1.2"), "{}", d.message);
    assert!(d.hint.as_ref().unwrap().contains("nc r.2"));
}

#[test]
fn nc_pins_are_fine_but_not_when_connected() {
    let ok = check("sheet \"A\" { r = R(1k)\n r.1 -- GND\n nc r.2 }");
    assert!(!has_errors(&ok.diags), "{:?}", codes(&ok, Severity::Error));
    let bad = check("sheet \"A\" { r = R(1k)\n r.1 -- GND\n r.2 -- GND\n nc r.2 }");
    assert!(codes(&bad, Severity::Error).contains(&"nc_connected"));
}

#[test]
fn shorted_rails_are_reported() {
    let c = check("sheet \"A\" { r = R(1k)\n +5V -- GND -- r.1\n r.2 -- GND }");
    assert!(codes(&c, Severity::Error).contains(&"rails_shorted"));
}

#[test]
fn two_outputs_conflict() {
    let src = r#"
part Buf { pins { 1 A : input  2 Y : output  3 VCC : power_in  4 GND : power_in } }
sheet "A" {
  a = Buf
  b = Buf
  a.Y -- b.Y -- X
  a.A, b.A -- IN
  r = R(1k)
  r.1 -- IN
  r.2 -- GND
  +5V -- a.VCC, b.VCC
  GND -- a.GND, b.GND
  power_flag +5V, GND
}
"#;
    let c = check(src);
    assert!(codes(&c, Severity::Error).contains(&"pin_conflict"), "{:?}", c.diags.iter().map(|d| &d.message).collect::<Vec<_>>());
}

#[test]
fn power_needs_a_driver_or_flag() {
    let src = "sheet \"A\" { u = NE555\n +5V -- u.VCC, u.R\n GND -- u.GND\n nc u.TR, u.THR, u.CV, u.Q, u.DIS }";
    let c = check(src);
    let d = c.diags.iter().find(|d| d.code == "power_not_driven").expect("power_not_driven");
    assert!(d.hint.as_ref().unwrap().contains("power_flag +5V"));
    let fixed = check(&src.replace(" }", "\n power_flag +5V, GND }"));
    assert!(!fixed.diags.iter().any(|d| d.code == "power_not_driven"));
}

#[test]
fn modules_expand_with_local_nets() {
    let src = r#"
module Div(top, mid) {
  a = R(10k)
  b = R(10k)
  top -- a.1
  a.2 -- tap -- b.1
  b.2 -- GND
  mid -- tap
}
sheet "A" {
  d1 = Div(top: +5V, mid: M1)
  d2 = Div(top: +5V, mid: M2)
  t1 = Conn_01x02
  t1.1 -- M1
  t1.2 -- M2
}
"#;
    let c = check(src);
    assert!(!has_errors(&c.diags), "{:?}", c.diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    let d = c.design.unwrap();
    assert_eq!(d.instances.len(), 5);
    // The two dividers' internal `tap` nets stay separate and take the sheet-level names.
    assert!(d.nets.iter().any(|n| n.name == "M1" && n.pins.len() == 3));
    assert!(d.nets.iter().any(|n| n.name == "M2" && n.pins.len() == 3));
    assert_eq!(d.groups.iter().filter(|g| g.framed).count(), 2);
    let (esch, _) = wirx::build(&d).unwrap();
    assert_eq!(esch["sheets"].as_array().unwrap().len(), 1);
}

#[test]
fn references_are_stable_and_explicit_refs_win() {
    let c = check("sheet \"A\" { a = R(1k)\n b = R(1k, ref: \"R1\")\n a.1, b.1 -- GND\n a.2, b.2 -- X }");
    let d = c.design.unwrap();
    assert_eq!(d.instances[0].reference, "R2");
    assert_eq!(d.instances[1].reference, "R1");
}

#[test]
fn footprint_aliases_expand() {
    let c = check("sheet \"A\" { c = C(1uF, 0402)\n c.1 -- X\n c.2 -- GND }");
    let d = c.design.unwrap();
    assert_eq!(d.instances[0].footprint, "Capacitor_SMD:C_0402_1005Metric");
}

#[test]
fn template_pin_count_mismatch_is_explained() {
    let c = check("part Q { symbol npn pins { 1 B 2 C } }");
    let d = c.diags.iter().find(|d| d.code == "template_pin_count").unwrap();
    assert!(d.message.contains("B, C, E"), "{}", d.message);
}
