use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use wirx::diag::{has_errors, Diagnostic, Severity, Sources};

#[derive(Parser)]
#[command(name = "wirx", version, about = "Schematic-as-code: run .wx (Rhai) circuit scripts into checked, laid-out schematics")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check a design (syntax, names, electrical rules) without drawing it.
    Check {
        file: PathBuf,
        /// Print diagnostics as JSON (for editors and AI tools).
        #[arg(long)]
        json: bool,
    },
    /// Check, lay out and write an eleceda schematic (.esch).
    Build {
        file: PathBuf,
        /// Output path (default: the input with .esch).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Write the schematic even when the checks report errors.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Draw the schematic as SVG files (one per sheet) for a quick look.
    Render {
        file: PathBuf,
        /// Output directory (default: next to the input).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Measure layout quality per group: crossings, bends, wire length, text collisions,
    /// unrouted pins, runtime and the placement candidate chosen.
    Stats {
        file: PathBuf,
        #[arg(long)]
        json: bool,
        /// Only the default placement (no candidate search).
        #[arg(long)]
        no_candidates: bool,
        /// No crossing-driven rerouting.
        #[arg(long)]
        no_reroute: bool,
        /// No cleanup (merging, jog removal, track separation).
        #[arg(long)]
        no_cleanup: bool,
    },
    /// Export the netlist for a PCB tool or for scripts.
    Netlist {
        file: PathBuf,
        #[arg(short, long, value_enum, default_value_t = NetFormat::Kicad)]
        format: NetFormat,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Turn an eleceda schematic (.esch) back into wirx source, e.g. after editing it in eleceda.
    Import {
        file: PathBuf,
        /// Output path (default: print the source).
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Write the source even when its netlist differs from the schematic's.
        #[arg(long)]
        force: bool,
        /// Write every part's reference, so parts added later take the next free number.
        #[arg(long)]
        pin_refs: bool,
    },
    /// Browse the standard library.
    Lib {
        #[command(subcommand)]
        cmd: LibCmd,
    },
}

#[derive(Subcommand)]
enum LibCmd {
    /// List parts and rails.
    List,
    /// Show one part's pins and defaults.
    Show { part: String },
    /// Write a design's symbols as a library for another tool: KiCad (.kicad_sym) or eleceda (.elib).
    Export {
        file: PathBuf,
        /// Output file; the extension picks the format (.kicad_sym or .elib).
        #[arg(short, long)]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum NetFormat {
    Kicad,
    Json,
}

fn report(sources: &Sources, diags: &[Diagnostic], json: bool) {
    if json {
        let list: Vec<_> = diags.iter().map(|d| sources.to_json(d)).collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap());
        return;
    }
    for d in diags {
        eprint!("{}", sources.render(d));
    }
    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = diags.iter().filter(|d| d.severity == Severity::Warning).count();
    if errors + warnings > 0 {
        eprintln!("{errors} error(s), {warnings} warning(s)");
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Check { file, json } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            report(&c.sources, &c.diags, json);
            if has_errors(&c.diags) {
                return ExitCode::FAILURE;
            }
            if !json && let Some(d) = &c.design {
                eprintln!("ok: {} parts, {} nets, {} sheet(s)", d.instances.len(), d.nets.len(), d.sheets.len());
            }
            ExitCode::SUCCESS
        }
        Cmd::Build { file, out, force, json } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            let blocked = has_errors(&c.diags) && (!force || c.design.is_none());
            if blocked {
                report(&c.sources, &c.diags, json);
                eprintln!("not written: fix the errors above (or pass --force)");
                return ExitCode::FAILURE;
            }
            let design = c.design.as_ref().unwrap();
            if design.instances.is_empty() {
                report(&c.sources, &c.diags, json);
                return fail("nothing to draw: add parts at the top level or in a `sheet` block");
            }
            match wirx::build(design) {
                Ok((esch, layout_diags)) => {
                    let mut all = c.diags.clone();
                    all.extend(layout_diags);
                    report(&c.sources, &all, json);
                    let out = out.unwrap_or_else(|| file.with_extension("esch"));
                    // The design's symbols go beside the schematic as a project library, so eleceda
                    // lists them (and keeps them) like any other library.
                    let mut esch = esch;
                    let lib_path = out.with_extension("elib");
                    let lib_name = lib_path.file_name().and_then(|n| n.to_str()).unwrap_or("wirx.elib").to_string();
                    if let Err(e) = std::fs::write(&lib_path, wirx::symbol::library::write_elib("wirx", &part_symbols(design))) {
                        return fail(&format!("cannot write {}: {e}", lib_path.display()));
                    }
                    esch["libraries"] = serde_json::json!([{ "name": "wirx", "path": lib_name }]);
                    if let Err(e) = std::fs::write(&out, serde_json::to_string_pretty(&esch).unwrap() + "\n") {
                        return fail(&format!("cannot write {}: {e}", out.display()));
                    }
                    if !json {
                        let sheets = esch["sheets"].as_array().map(|a| a.len()).unwrap_or(0);
                        eprintln!("wrote {} ({} parts, {} nets, {} sheet(s))", out.display(), design.instances.len(), design.nets.len(), sheets);
                    }
                    ExitCode::SUCCESS
                }
                Err(problems) => {
                    report(&c.sources, &c.diags, json);
                    for p in problems {
                        eprintln!("internal layout error: {p}");
                    }
                    fail("the drawing does not match the design (this is a wirx bug; please report it with the source)")
                }
            }
        }
        Cmd::Netlist { file, format, out } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            report(&c.sources, &c.diags, false);
            let Some(design) = c.design.as_ref().filter(|_| !has_errors(&c.diags)) else {
                return ExitCode::FAILURE;
            };
            let text = match format {
                NetFormat::Kicad => wirx::backend::netlist::kicad(design, &file.display().to_string()),
                NetFormat::Json => serde_json::to_string_pretty(&wirx::backend::netlist::json(design)).unwrap() + "\n",
            };
            match out {
                Some(p) => {
                    if let Err(e) = std::fs::write(&p, text) {
                        return fail(&format!("cannot write {}: {e}", p.display()));
                    }
                }
                None => print!("{text}"),
            }
            ExitCode::SUCCESS
        }
        Cmd::Render { file, out } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            report(&c.sources, &c.diags, false);
            let Some(design) = c.design.as_ref().filter(|d| !d.instances.is_empty()) else {
                return fail("nothing to draw");
            };
            match wirx::draw(design) {
                Ok((sch, diags)) => {
                    report(&c.sources, &diags, false);
                    let dir = out.unwrap_or_else(|| file.parent().map(Path::to_path_buf).unwrap_or_default());
                    let _ = std::fs::create_dir_all(&dir);
                    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("design");
                    for (name, svg) in wirx::backend::svg::render(design, &sch) {
                        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                        let path = dir.join(format!("{stem}-{safe}.svg"));
                        if let Err(e) = std::fs::write(&path, svg) {
                            return fail(&format!("cannot write {}: {e}", path.display()));
                        }
                        eprintln!("wrote {}", path.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(problems) => {
                    for p in problems {
                        eprintln!("internal layout error: {p}");
                    }
                    ExitCode::FAILURE
                }
            }
        }
        Cmd::Import { file, out, force, pin_refs } => import(&file, out.as_deref(), force, pin_refs),
        Cmd::Stats { file, json, no_candidates, no_reroute, no_cleanup } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            let Some(design) = c.design.as_ref().filter(|d| !wirx::diag::has_errors(&c.diags) && !d.instances.is_empty()) else {
                report(&c.sources, &c.diags, false);
                return fail("the design has errors");
            };
            let opts = wirx::layout::Options { candidates: !no_candidates, reroute: !no_reroute, cleanup: !no_cleanup };
            let start = std::time::Instant::now();
            let sch = match wirx::draw_with(design, &opts) {
                Ok((s, _)) => s,
                Err(p) => return fail(&format!("layout failed verification: {}", p.join("; "))),
            };
            let total_ms = start.elapsed().as_secs_f64() * 1000.0;
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "groups": sch.stats, "total_ms": total_ms })).unwrap());
            } else {
                println!("{:<16} {:>4} {:>9} {:>5} {:>7} {:>5} {:>5} {:>4} {:>10} {:>8} {:>8} {:>9}", "group", "cand", "crossings", "bends", "length", "wires", "hugs", "4way", "collisions", "unrouted", "moved", "ms");
                let mut t = (0, 0, 0i64, 0, 0);
                for g in &sch.stats {
                    println!(
                        "{:<16} {:>4} {:>9} {:>5} {:>7} {:>5} {:>5} {:>4} {:>10} {:>8} {:>8} {:>9.1}",
                        g.group, format!("{}/{}", g.candidate, g.candidates_tried), g.crossings, g.bends, g.length, g.wires, g.hugs, g.four_way, g.collisions, g.unrouted, g.displacement, g.micros as f64 / 1000.0
                    );
                    t = (t.0 + g.crossings, t.1 + g.bends, t.2 + g.length, t.3 + g.collisions, t.4 + g.unrouted);
                }
                println!("{:<16} {:>4} {:>9} {:>5} {:>7} {:>5} {:>5} {:>4} {:>10} {:>8} {:>8} {:>9.1}", "total", "", t.0, t.1, t.2, "", "", "", t.3, t.4, "", total_ms);
            }
            ExitCode::SUCCESS
        }
        Cmd::Lib { cmd } => lib(cmd),
    }
}

fn import(file: &Path, out: Option<&Path>, force: bool, pin_refs: bool) -> ExitCode {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return fail(&format!("cannot read {}: {e}", file.display())),
    };
    let esch: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return fail(&format!("{} is not valid JSON: {e}", file.display())),
    };
    let name = out.unwrap_or(file).file_stem().and_then(|s| s.to_str()).unwrap_or("design").to_string();
    let imported = match wirx::import::import_with(&esch, &name, wirx::import::ImportOptions { pin_refs }) {
        Ok(i) => i,
        Err(e) => return fail(&e),
    };
    for w in &imported.warnings {
        eprintln!("warning: {w}");
    }
    if !imported.mismatches.is_empty() {
        for p in &imported.mismatches {
            eprintln!("mismatch: {p}");
        }
        if !force {
            return fail("the source does not reproduce the schematic's netlist (this is a wirx bug; pass --force to write it anyway)");
        }
    }
    match out {
        Some(p) => {
            if let Err(e) = std::fs::write(p, &imported.text) {
                return fail(&format!("cannot write {}: {e}", p.display()));
            }
            eprintln!("wrote {}", p.display());
        }
        None => print!("{}", imported.text),
    }
    ExitCode::SUCCESS
}

fn lib(cmd: LibCmd) -> ExitCode {
    use wirx::stdlib::{PARTS, RAILS};
    match cmd {
        LibCmd::List => {
            println!("Parts (call as Name(value, footprint, options); all arguments optional):");
            for p in PARTS {
                println!("  {:<14} {}", p.name, p.description);
            }
            println!("  {:<14} single-row header with n pins", "Conn(n)");
            println!("  {:<14} dual-row header, 2 x rows pins", "Conn2(rows)");
            println!("
Rails:");
            for (konst, name, kind) in RAILS {
                println!("  {konst:<14} {name} ({kind:?})");
            }
            println!("  {:<14} make your own: supply(\"VIO\"), ground(\"GND_ISO\"), negative(\"-15V\")", "");
            println!("
Symbol shapes: box, {}", wirx::symbol::templates::TEMPLATE_NAMES.join(", "));
            ExitCode::SUCCESS
        }
        LibCmd::Export { file, out } => {
            let c = match wirx::check_file(&file) {
                Ok(c) => c,
                Err(e) => return fail(&e),
            };
            let Some(design) = c.design.as_ref().filter(|_| !wirx::diag::has_errors(&c.diags)) else {
                report(&c.sources, &c.diags, false);
                return fail("the design has errors");
            };
            let symbols = part_symbols(design);
            let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let text = match ext.as_str() {
                "kicad_sym" => wirx::symbol::library::write_kicad_sym(&symbols),
                "elib" => wirx::symbol::library::write_elib("wirx", &symbols),
                _ => return fail("the output must end in .kicad_sym or .elib"),
            };
            if let Err(e) = std::fs::write(&out, text) {
                return fail(&format!("cannot write {}: {e}", out.display()));
            }
            eprintln!("wrote {} ({} symbols)", out.display(), symbols.len());
            ExitCode::SUCCESS
        }
        LibCmd::Show { part } => {
            let Some(p) = wirx::stdlib::find(&part) else {
                return fail(&format!("no part `{part}` in the standard library (see `wirx lib list`)"));
            };
            let d = p.to_def();
            println!("{}()  prefix {}, symbol {}", d.name, d.prefix, d.symbol);
            if !d.description.is_empty() {
                println!("  {}", d.description);
            }
            println!("  value     {}", d.value);
            println!("  footprint {}", d.footprint);
            println!("  pins:");
            for pin in &d.pins {
                println!("    {:<5} {:<10} {}", pin.number, pin.name, pin.ptype.name());
            }
            ExitCode::SUCCESS
        }
    }
}

/// The symbols of a design's parts (power symbols are left out: every tool has its own).
fn part_symbols(design: &wirx::ir::Design) -> Vec<wirx::symbol::SymbolDef> {
    let mut seen = std::collections::HashSet::new();
    design.symbols.iter().filter(|s| !s.is_power() && seen.insert(s.id.clone())).cloned().collect()
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}
