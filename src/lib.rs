//! wirx: schematic-as-code. Describe parts, nets and groups in a small language; the compiler
//! checks the circuit, lays out the schematic and writes it for eleceda (`.esch`) or as a netlist.

pub mod backend;
pub mod diag;
pub mod erc;
pub mod geom;
pub mod ir;
pub mod layout;
pub mod sema;
pub mod symbol;
pub mod syntax;
pub mod verify;

use diag::{Diagnostic, Sources};
use std::path::Path;

pub struct Compiled {
    pub sources: Sources,
    pub design: Option<ir::Design>,
    pub diags: Vec<Diagnostic>,
}

/// Parse, elaborate and check a design given as text.
pub fn check_source(name: &str, text: &str, base_dir: &Path) -> Compiled {
    let mut sources = Sources::default();
    let program = match sema::load(&mut sources, name, text, base_dir) {
        Ok(p) => p,
        Err(diags) => return Compiled { sources, design: None, diags },
    };
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("design").to_string();
    let (design, mut diags) = sema::elaborate(&program, &stem);
    if !diag::has_errors(&diags) {
        diags.extend(erc::check(&design));
    }
    diags.sort_by_key(|d| (d.severity, d.span.map(|s| (s.file, s.start))));
    Compiled { sources, design: Some(design), diags }
}

/// Parse, elaborate and check a design file.
pub fn check_file(path: &Path) -> Result<Compiled, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(check_source(&path.display().to_string(), &text, &base))
}

/// Lay out a checked design; the drawing is verified against the design before it is returned.
pub fn draw(design: &ir::Design) -> Result<(layout::Schematic, Vec<Diagnostic>), Vec<String>> {
    let (schematic, diags) = layout::layout(design);
    let problems = verify::check(design, &schematic);
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok((schematic, diags))
}

/// Lay out a checked design and produce the `.esch` JSON.
pub fn build(design: &ir::Design) -> Result<(serde_json::Value, Vec<Diagnostic>), Vec<String>> {
    let (schematic, diags) = draw(design)?;
    Ok((backend::esch::to_esch(design, &schematic), diags))
}
