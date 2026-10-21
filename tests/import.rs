//! `.esch` → `.wx` round trip: build each example, import the schematic, and check that the imported
//! source has the same netlist as the original.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn wirx(args: &[&Path]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wirx")).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "wirx {args:?} failed:\n{stderr}");
    stderr
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wirx-import-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Nets as sets of `REF.pin`.
fn nets(netlist: &Path) -> BTreeSet<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(netlist).unwrap()).unwrap();
    v["nets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["pins"].as_array().unwrap().iter().map(|p| format!("{}.{}", p["ref"].as_str().unwrap(), p["pin"].as_str().unwrap())).collect())
        .collect()
}

#[test]
fn examples_round_trip_through_esch() {
    for name in ["ldo", "blinker", "mcu"] {
        let dir = scratch(name);
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(format!("{name}.wx"));
        let (esch, wx) = (dir.join(format!("{name}.esch")), dir.join(format!("{name}.wx")));
        let (net_a, net_b) = (dir.join("a.json"), dir.join("b.json"));
        wirx(&[Path::new("build"), &src, Path::new("-o"), &esch]);
        let warnings = wirx(&[Path::new("import"), &esch, Path::new("-o"), &wx]);
        assert!(!warnings.contains("warning"), "{name}: {warnings}");
        wirx(&[Path::new("netlist"), &src, Path::new("--format"), Path::new("json"), Path::new("-o"), &net_a]);
        wirx(&[Path::new("netlist"), &wx, Path::new("--format"), Path::new("json"), Path::new("-o"), &net_b]);
        let text = std::fs::read_to_string(&wx).unwrap();
        assert_eq!(nets(&net_a), nets(&net_b), "{name}: netlists differ; imported source:\n{text}");

        // The imported source builds cleanly, and importing its schematic gives the same source.
        let esch2 = dir.join("again.esch");
        let wx2 = dir.join(format!("{name}.again.wx"));
        wirx(&[Path::new("build"), &wx, Path::new("-o"), &esch2]);
        wirx(&[Path::new("import"), &esch2, Path::new("-o"), &wx2]);
        assert_eq!(text, std::fs::read_to_string(&wx2).unwrap(), "{name}: import is not stable");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn edits_made_in_eleceda_come_back() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ldo.wx");
    let c = wirx::check_file(&src).unwrap();
    let (mut esch, _) = wirx::build(c.design.as_ref().unwrap()).unwrap();
    // Change R1's value and give C2 a part number, the way an edit in eleceda would.
    for sheet in esch["sheets"].as_array_mut().unwrap() {
        for it in sheet["items"].as_array_mut().unwrap() {
            let Some(fields) = it["fields"].as_array_mut() else { continue };
            let reference = fields.iter().find(|f| f["name"] == "Reference").unwrap()["value"].clone();
            if reference == "R1" {
                fields.iter_mut().find(|f| f["name"] == "Value").unwrap()["value"] = "4k7".into();
            }
            if reference == "C2" {
                fields.push(serde_json::json!({ "name": "MPN", "value": "GRM21BR61A226ME44", "at": { "x": 0, "y": 0 }, "visible": false }));
            }
        }
    }
    let imported = wirx::import::import(&esch, "ldo").unwrap();
    assert!(imported.mismatches.is_empty(), "{:?}", imported.mismatches);
    assert!(imported.text.contains(r#"R("4k7")"#), "{}", imported.text);
    assert!(imported.text.contains(r#"MPN: "GRM21BR61A226ME44""#), "{}", imported.text);
    let c = wirx::check_source("ldo.wx", &imported.text, Path::new("."));
    let d = c.design.unwrap();
    let r1 = d.instances.iter().find(|i| i.reference == "R1").unwrap();
    assert_eq!(r1.value, "4k7");
}

#[test]
fn custom_parts_keep_their_references_and_box_symbols() {
    // chessboard-mini has a 4-pin box oscillator (once mistaken for the `resistor` template) and
    // custom chips whose references automatic numbering would swap.
    let esch = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chessboard-mini.esch");
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&esch).unwrap()).unwrap();
    let imported = wirx::import::import(&value, "chessboard-mini").expect("imports");
    assert!(imported.mismatches.is_empty(), "{:?}", imported.mismatches);
    assert!(!imported.warnings.iter().any(|w| w.contains("becomes")), "{:?}", imported.warnings);
    assert!(imported.text.contains(".options(#{ ref: "), "custom parts pin their references");
    let refs = |v: &serde_json::Value| -> BTreeSet<String> {
        v["sheets"].as_array().unwrap().iter().flat_map(|s| s["items"].as_array().unwrap().iter())
            .filter(|i| i["kind"] == "symbol")
            .filter_map(|i| i["fields"].as_array().unwrap().iter().find(|f| f["name"] == "Reference").and_then(|f| f["value"].as_str()).map(String::from))
            .filter(|r| !r.starts_with('#'))
            .collect()
    };
    let c = wirx::check_source("chessboard-mini.wx", &imported.text, Path::new("."));
    let (rebuilt, _) = wirx::build(c.design.as_ref().expect("compiles")).expect("builds");
    assert_eq!(refs(&value), refs(&rebuilt));
}

#[test]
fn pinned_references_survive_an_added_part() {
    // With every reference written, a part added before the others takes the next free number
    // instead of renumbering the rest (what an editor or an AI agent needs).
    let esch = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chessboard-mini.esch");
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&esch).unwrap()).unwrap();
    let imported = wirx::import::import_with(&value, "chessboard-mini", wirx::import::ImportOptions { pin_refs: true }).expect("imports");
    assert!(imported.mismatches.is_empty(), "{:?}", imported.mismatches);
    let refs = |src: &str| -> Vec<String> {
        let c = wirx::check_source("t.wx", src, Path::new("."));
        let d = c.design.expect("compiles");
        let mut v: Vec<String> = d.instances.iter().map(|i| i.reference.clone()).collect();
        v.sort();
        v
    };
    let before = refs(&imported.text);
    // A new resistor near the top of the source, before every other part.
    let at = imported.text.find("sheet(").unwrap();
    let edited = format!("{}let extra = R(\"10k\");\nextra[1] -> P3V3;\nextra[2] -> GND;\n{}", &imported.text[..at], &imported.text[at..]);
    let after = refs(&edited);
    assert_eq!(after.len(), before.len() + 1);
    for r in &before {
        assert!(after.contains(r), "{r} was renumbered");
    }
}

#[test]
fn power_flags_survive_import_and_rebuild() {
    // A rail's flag must not be given to a capacitor drawn in a bank (where it is never drawn).
    let esch = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chessboard-mini.esch");
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&esch).unwrap()).unwrap();
    let flags = |v: &serde_json::Value| {
        v["sheets"].as_array().unwrap().iter().flat_map(|s| s["items"].as_array().unwrap().iter()).filter(|i| i["kind"] == "symbol" && i["libId"].as_str().unwrap_or("").to_lowercase().contains("flag")).count()
    };
    let imported = wirx::import::import_with(&value, "chessboard-mini", wirx::import::ImportOptions { pin_refs: true }).expect("imports");
    let c = wirx::check_source("chessboard-mini.wx", &imported.text, Path::new("."));
    let (rebuilt, _) = wirx::build(c.design.as_ref().expect("compiles")).expect("builds");
    assert_eq!(flags(&rebuilt), flags(&value));
}

/// Where pin `number` of part `reference` was drawn.
fn pin_at(s: &wirx::layout::Schematic, d: &wirx::ir::Design, reference: &str, number: &str) -> wirx::geom::Pt {
    let i = d.instances.iter().position(|x| x.reference == reference).unwrap_or_else(|| panic!("no {reference}"));
    for it in s.sheets.iter().flat_map(|sh| &sh.items) {
        if let wirx::layout::Item::Symbol { lib_id, at, orient, inst: Some(j), .. } = it
            && *j == i
        {
            let sym = s.symbols.iter().find(|x| &x.id == lib_id).unwrap();
            return wirx::symbol::place_pin(sym.pins.iter().find(|p| p.number == number).unwrap(), *orient, *at).at;
        }
    }
    panic!("{reference} not drawn")
}

/// Is there a wire path between `a` and `b` on one sheet?
fn wired(s: &wirx::layout::Schematic, a: wirx::geom::Pt, b: wirx::geom::Pt) -> bool {
    use wirx::geom::Pt;
    let on = |p: Pt, (x, y): (Pt, Pt)| (x.x == y.x || x.y == y.y) && p.x >= x.x.min(y.x) && p.x <= x.x.max(y.x) && p.y >= x.y.min(y.y) && p.y <= x.y.max(y.y);
    s.sheets.iter().any(|sheet| {
        let wires: Vec<(Pt, Pt)> = sheet.items.iter().filter_map(|it| if let wirx::layout::Item::Wire { a, b } = it { Some((*a, *b)) } else { None }).collect();
        let mut reached: Vec<bool> = wires.iter().map(|&w| on(a, w)).collect();
        while let Some(i) = (0..wires.len()).find(|&i| {
            !reached[i] && (0..wires.len()).any(|j| reached[j] && [wires[i].0, wires[i].1].iter().any(|&p| on(p, wires[j])) || reached[j] && [wires[j].0, wires[j].1].iter().any(|&p| on(p, wires[i])))
        }) {
            reached[i] = true;
        }
        wires.iter().zip(&reached).any(|(&w, &r)| r && on(b, w))
    })
}

#[test]
fn imported_decoupling_stays_at_stacked_pins() {
    // RP2350A in the imported chessboard-mini: DVDD is pins 6, 23 and 39 drawn as one (23 and 39
    // hidden behind 6), between VREG_VIN and the IOVDD pins on a crowded top edge, with the core
    // regulator's inductor hanging off VREG_LX next to them. Each capacitor's `near` pin keeps its
    // bank: drawn there and wired to it, not left in a row under the chip.
    let esch = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/chessboard-mini.esch");
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&esch).unwrap()).unwrap();
    let imported = wirx::import::import_with(&value, "chessboard-mini", wirx::import::ImportOptions { pin_refs: true }).expect("imports");
    let c = wirx::check_source("chessboard-mini.wx", &imported.text, Path::new("."));
    let d = c.design.expect("compiles");
    let (s, _) = wirx::draw(&d).unwrap_or_else(|e| panic!("{e:#?}"));
    for (cap, pin) in [("C13", "49"), ("C15", "6"), ("C16", "6"), ("C17", "6"), ("C18", "6")] {
        assert!(wired(&s, pin_at(&s, &d, cap, "1"), pin_at(&s, &d, "U3", pin)), "{cap} is wired to U3 pin {pin}");
    }
    for hidden in ["23", "39"] {
        assert_eq!(pin_at(&s, &d, "U3", hidden), pin_at(&s, &d, "U3", "6"), "DVDD pin {hidden} behind pin 6");
    }
}
