use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use wirx::diag::{has_errors, Diagnostic, Severity, Sources};

#[derive(Parser)]
#[command(name = "wirx", version, about = "Schematic-as-code: compile .wx circuits into checked, laid-out schematics")]
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
    /// Export the netlist for a PCB tool or for scripts.
    Netlist {
        file: PathBuf,
        #[arg(short, long, value_enum, default_value_t = NetFormat::Kicad)]
        format: NetFormat,
        #[arg(short, long)]
        out: Option<PathBuf>,
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
                    for (name, svg) in wirx::backend::svg::render(&sch) {
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
        Cmd::Lib { cmd } => lib(cmd),
    }
}

fn lib(cmd: LibCmd) -> ExitCode {
    let mut sources = Sources::default();
    let program = match wirx::sema::load(&mut sources, "<lib>", "", Path::new(".")) {
        Ok(p) => p,
        Err(_) => return fail("standard library failed to load"),
    };
    let std_ast = &program.files[0].1;
    match cmd {
        LibCmd::List => {
            println!("Parts:");
            for item in &std_ast.items {
                if let wirx::syntax::ast::Item::Part(p) = item {
                    let desc = p.props.iter().find(|(k, _)| k.text == "description").and_then(|(_, v)| v.as_ref()).map(|v| v.text.clone()).unwrap_or_default();
                    println!("  {:<14} {desc}", p.name.text);
                }
            }
            println!("  {:<14} single-row header with NN pins (generated)", "Conn_01xNN");
            println!("  {:<14} dual-row header, 2×NN pins (generated)", "Conn_02xNN");
            println!("\nRails:");
            for item in &std_ast.items {
                if let wirx::syntax::ast::Item::Rail(r) = item {
                    println!("  {:<14} {}", r.name.text, r.kind.text);
                }
            }
            println!("  {:<14} any name starting with + or - (e.g. +3V3, -12V)", "+X / -X");
            println!("\nSymbol shapes: {}", wirx::symbol::templates::TEMPLATE_NAMES.join(", "));
            ExitCode::SUCCESS
        }
        LibCmd::Show { part } => {
            let text = format!("sheet \"x\" {{ p = {part} }}");
            let c = wirx::check_source("<lib>", &text, Path::new("."));
            let Some(d) = c.design.filter(|d| !d.parts.is_empty()) else {
                return fail(&format!("no part `{part}` in the standard library"));
            };
            let p = &d.parts[0];
            println!("part {}  (prefix {}, symbol {})", p.name, p.prefix, p.symbol);
            if !p.description.is_empty() {
                println!("  {}", p.description);
            }
            println!("  value     {}", p.value);
            println!("  footprint {}", p.footprint);
            println!("  pins:");
            for pin in &p.pins {
                println!("    {:<5} {:<10} {}", pin.number, pin.name, pin.ptype.name());
            }
            ExitCode::SUCCESS
        }
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}
