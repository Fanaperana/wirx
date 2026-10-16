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
