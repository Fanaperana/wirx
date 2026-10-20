//! wirx for JavaScript. Every function takes and returns JSON strings, so the binding stays small
//! and any host (a browser, a Web Worker, a webview) can call it without extra glue.
//!
//! There is no filesystem here: `import "…"` and `library: "…"` references to other files report
//! an error instead of reading them.

use serde::Serialize;
use std::path::Path;
use wasm_bindgen::prelude::*;
use wirx::diag::{has_errors, Diagnostic, JsonDiagnostic, Sources};

#[derive(Serialize)]
struct CheckOut {
    ok: bool,
    diagnostics: Vec<JsonDiagnostic>,
}

#[derive(Serialize)]
struct BuildOut {
    ok: bool,
    /// The `.esch` project when the build succeeded.
    esch: Option<serde_json::Value>,
    diagnostics: Vec<JsonDiagnostic>,
    /// Internal layout problems (the drawing did not match the design), when any.
    errors: Vec<String>,
}

#[derive(Serialize)]
struct ImportOut {
    ok: bool,
    /// The `.wx` source.
    source: Option<String>,
    /// What the source cannot say (notes, buses, some fields).
    warnings: Vec<String>,
    /// Connectivity that the source would change; empty when faithful.
    mismatches: Vec<String>,
    error: Option<String>,
}

fn json(diags: &[Diagnostic], sources: &Sources) -> Vec<JsonDiagnostic> {
    diags.iter().map(|d| sources.to_json(d)).collect()
}

fn to_string<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{e}\"}}"))
}

/// Run a design script and its checks. Returns `{ ok, diagnostics }`.
#[wasm_bindgen]
pub fn check(name: &str, source: &str) -> String {
    let c = wirx::check_source(name, source, Path::new("."));
    to_string(&CheckOut { ok: c.design.is_some() && !has_errors(&c.diags), diagnostics: json(&c.diags, &c.sources) })
}

/// Check, lay out and draw a design. Returns `{ ok, esch, diagnostics, errors }`; `esch` is the
/// eleceda project (with its symbols embedded) when there are no errors.
#[wasm_bindgen]
pub fn build(name: &str, source: &str) -> String {
    let c = wirx::check_source(name, source, Path::new("."));
    let mut out = BuildOut { ok: false, esch: None, diagnostics: json(&c.diags, &c.sources), errors: vec![] };
    let Some(design) = c.design.as_ref().filter(|_| !has_errors(&c.diags)) else {
        return to_string(&out);
    };
    if design.instances.is_empty() {
        out.errors.push("nothing to draw: add parts at the top level or in a `sheet` block".into());
        return to_string(&out);
    }
    match wirx::build(design) {
        Ok((esch, layout_diags)) => {
            let mut all = c.diags.clone();
            all.extend(layout_diags);
            out.diagnostics = json(&all, &c.sources);
            out.esch = Some(esch);
            out.ok = true;
        }
        Err(problems) => out.errors = problems,
    }
    to_string(&out)
}

/// Turn an eleceda project (`.esch` JSON text) back into wirx source. Returns
/// `{ ok, source, warnings, mismatches, error }`; `ok` is false when the source would not
/// reproduce the schematic's connectivity.
#[wasm_bindgen(js_name = importEsch)]
pub fn import_esch(esch: &str, design_name: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(esch) {
        Ok(v) => v,
        Err(e) => return to_string(&ImportOut { ok: false, source: None, warnings: vec![], mismatches: vec![], error: Some(format!("not JSON: {e}")) }),
    };
    match wirx::import::import(&value, design_name) {
        Ok(i) => to_string(&ImportOut { ok: i.mismatches.is_empty(), source: Some(i.text), warnings: i.warnings, mismatches: i.mismatches, error: None }),
        Err(e) => to_string(&ImportOut { ok: false, source: None, warnings: vec![], mismatches: vec![], error: Some(e) }),
    }
}

/// The wirx version this module was built from.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}
