//! eleceda `.esch` writer (`packages/core/src/model/schematic.ts`, format version 2).
//!
//! Item ids are UUIDv5 values derived from what the item is (instance path, wire geometry, …), so
//! recompiling an unchanged design gives a byte-identical file and small edits give small diffs.

use crate::geom::Pt;
use crate::ir::Design;
use crate::layout::{Item, Schematic};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use uuid::Uuid;

const NAMESPACE: Uuid = Uuid::from_u128(0x5f1c_9a2e_7b3d_4e8f_a1c0_77d2_e9b4_0a61);

struct Ids(HashSet<Uuid>);

impl Ids {
    fn get(&mut self, key: &str) -> String {
        let mut id = Uuid::new_v5(&NAMESPACE, key.as_bytes());
        let mut n = 1;
        while !self.0.insert(id) {
            id = Uuid::new_v5(&NAMESPACE, format!("{key}#{n}").as_bytes());
            n += 1;
        }
        id.to_string()
    }
}

fn p(v: Pt) -> Value {
    json!({ "x": v.x, "y": v.y })
}

pub fn to_esch(d: &Design, s: &Schematic) -> Value {
    let mut ids = Ids(HashSet::new());
    let mut pwr = 0;
    let mut flg = 0;
    let mut sheets = Vec::new();
    for sheet in &s.sheets {
        let sheet_id = ids.get(&format!("sheet:{}", sheet.name));
        let mut items = Vec::new();
        for it in &sheet.items {
            let v = match it {
                Item::Symbol { key, lib_id, at, orient, fields, inst } => {
                    let mut fields: Vec<Value> = fields.iter().map(|f| serde_json::to_value(f).unwrap()).collect();
                    if inst.is_none() {
                        // Number power symbols and flags (#PWR01, #FLG01) so nothing reads as unannotated.
                        for f in &mut fields {
                            if f["name"] == "Reference" {
                                let r = f["value"].as_str().unwrap_or("").to_string();
                                if r.starts_with("#PWR") {
                                    pwr += 1;
                                    f["value"] = json!(format!("#PWR{pwr:02}"));
                                } else if r.starts_with("#FLG") {
                                    flg += 1;
                                    f["value"] = json!(format!("#FLG{flg:02}"));
                                }
                            }
                        }
                    }
                    let mut m = Map::new();
                    m.insert("kind".into(), json!("symbol"));
                    m.insert("id".into(), json!(ids.get(key)));
                    m.insert("libId".into(), json!(lib_id));
                    m.insert("at".into(), p(*at));
                    m.insert("rot".into(), json!(orient.rot));
                    m.insert("mirror".into(), json!(orient.mirror));
                    m.insert("unit".into(), json!(1));
                    m.insert("fields".into(), Value::Array(fields));
                    if let Some(i) = inst
                        && d.instances[*i].dnp
                    {
                        m.insert("dnp".into(), json!(true));
                    }
                    Value::Object(m)
                }
                Item::Wire { a, b } => json!({
                    "kind": "wire",
                    "id": ids.get(&format!("wire:{}:{},{}:{},{}", sheet.name, a.x, a.y, b.x, b.y)),
                    "a": p(*a), "b": p(*b)
                }),
                Item::Junction(at) => json!({
                    "kind": "junction", "id": ids.get(&format!("junction:{}:{},{}", sheet.name, at.x, at.y)), "at": p(*at)
                }),
                Item::NoConnect(at) => json!({
                    "kind": "noconnect", "id": ids.get(&format!("nc:{}:{},{}", sheet.name, at.x, at.y)), "at": p(*at)
                }),
                Item::Label { text, at, rot, global, shape, .. } => {
                    let mut v = json!({
                        "kind": "label",
                        "id": ids.get(&format!("label:{}:{}:{},{}", sheet.name, text, at.x, at.y)),
                        "labelType": if *global { "global" } else { "local" },
                        "text": text,
                        "at": p(*at),
                        "rot": rot
                    });
                    if *global {
                        v["shape"] = json!(shape);
                    }
                    v
                }
                Item::Block { name, rect } => json!({
                    "kind": "block",
                    "id": ids.get(&format!("block:{}:{}", sheet.name, name)),
                    "name": name,
                    "at": { "x": rect.x0, "y": rect.y0 },
                    "size": { "w": rect.w(), "h": rect.h() }
                }),
            };
            items.push(v);
        }
        sheets.push(json!({ "id": sheet_id, "name": sheet.name, "paper": sheet.paper.name(), "items": items }));
    }
    let mut symbols = Map::new();
    for sym in &s.symbols {
        symbols.insert(sym.id.clone(), serde_json::to_value(sym).unwrap());
    }
    let mut project = json!({
        "format": "eleceda-schematic",
        "version": 2,
        "name": d.name,
        "sheets": sheets,
        "symbols": symbols,
    });
    let mut tb = Map::new();
    tb.insert("title".into(), json!(d.meta.get("title").cloned().unwrap_or_else(|| d.name.clone())));
    for k in ["revision", "company", "author", "date", "comment"] {
        if let Some(v) = d.meta.get(k) {
            tb.insert(k.into(), json!(v));
        }
    }
    project["titleBlock"] = Value::Object(tb);
    project
}
