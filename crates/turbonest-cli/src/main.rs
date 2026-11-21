//! `turbonest` command-line tool.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use turbonest_core::export;
use turbonest_core::geom::{BBox, FlattenMode};
use turbonest_core::import::{ImportOptions, UnitsOverride, import_dxf};
use turbonest_core::job::{load_job, load_materials, read_job_csv};
use turbonest_core::model::{Material, PlyShape};
use turbonest_core::verify::verify;
use turbonest_nest::{NestOptions, nest};

#[derive(Parser)]
#[command(name = "turbonest", version, about = "Nest prepreg plies on a roll, keeping every ply's fibre direction.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum UnitsArg {
    Auto,
    Mm,
    Cm,
    M,
    Inch,
}

impl From<UnitsArg> for UnitsOverride {
    fn from(u: UnitsArg) -> Self {
        match u {
            UnitsArg::Auto => UnitsOverride::Auto,
            UnitsArg::Mm => UnitsOverride::Mm,
            UnitsArg::Cm => UnitsOverride::Cm,
            UnitsArg::M => UnitsOverride::M,
            UnitsArg::Inch => UnitsOverride::Inch,
        }
    }
}

#[derive(clap::Args, Clone)]
struct ImportArgs {
    /// Units of the DXF files (auto reads $INSUNITS, falls back to mm).
    #[arg(long, value_enum, default_value_t = UnitsArg::Auto)]
    units: UnitsArg,
    /// Endpoints closer than this are joined, mm.
    #[arg(long, default_value_t = 0.01)]
    join_tol: f64,
    /// Gaps up to this size are bridged with a warning, mm.
    #[arg(long, default_value_t = 0.1)]
    gap_tol: f64,
}

impl ImportArgs {
    fn options(&self) -> ImportOptions {
        ImportOptions { units: self.units.into(), join_tol: self.join_tol, gap_tol: self.gap_tol, ..ImportOptions::default() }
    }
}

#[derive(clap::Args, Clone)]
struct JobArgs {
    /// Job CSV (columns: file, ref, variants, cosmetic, mirror, kit, name).
    job: PathBuf,
    /// Material library (TOML). Default: materials.toml next to the job CSV.
    #[arg(long)]
    materials: Option<PathBuf>,
    /// Material name from the library.
    #[arg(long, short)]
    material: String,
    /// Output folder.
    #[arg(long, short, default_value = "out")]
    out: PathBuf,
    #[command(flatten)]
    import: ImportArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// Nest a job and write layout.dxf, layout.svg, plies.csv and report.json.
    Nest {
        #[command(flatten)]
        job: JobArgs,
        /// Optimisation time in seconds.
        #[arg(long, short, default_value_t = 60)]
        time: u64,
        /// Random seed (same seed + same input = same layout).
        #[arg(long)]
        seed: Option<u64>,
        /// Worker threads (default: all cores).
        #[arg(long)]
        workers: Option<usize>,
        /// Stop early when the search stops improving.
        #[arg(long)]
        early_stop: bool,
        /// Also write the solver instance JSON (for debugging with the sparrow CLI).
        #[arg(long)]
        dump_instance: bool,
    },
    /// Import a job and write orientation.svg (0° axes and fibre directions) without nesting.
    Check {
        #[command(flatten)]
        job: JobArgs,
    },
    /// List the parts found in DXF files: size, edges, fibre-layer lines, warnings.
    Inspect {
        files: Vec<PathBuf>,
        #[command(flatten)]
        import: ImportArgs,
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).format_timestamp(None).init();
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Inspect { files, import } => inspect(&files, &import.options()),
        Cmd::Check { job } => {
            let (plies, material, _) = prepare(&job)?;
            println!("{} ply shapes, material {} ({:.0} mm roll, {:.0} mm usable)", plies.len(), material.name, material.roll_width, material.usable_width());
            print_ply_table(&plies);
            println!("\nwrote {}", job.out.join("orientation.svg").display());
            Ok(0)
        }
        Cmd::Nest { job, time, seed, workers, early_stop, dump_instance } => {
            let (plies, material, warnings) = prepare(&job)?;
            print_ply_table(&plies);
            let seed = seed.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs()));
            let opts = NestOptions {
                time: Duration::from_secs(time),
                seed: Some(seed),
                workers: workers.unwrap_or(NestOptions::default().workers),
                early_stop,
                ..NestOptions::default()
            };
            if dump_instance {
                let groups = turbonest_nest::build_groups(&plies, &material)?;
                let inst = turbonest_nest::build_instance(&plies, &material, &groups, opts.nest_tol);
                let p = job.out.join("solver_instance.json");
                std::fs::write(&p, serde_json::to_string_pretty(&inst)?)?;
                println!("wrote {}", p.display());
            }
            let total: u32 = plies.iter().flat_map(|p| p.variants.iter().map(|v| v.qty)).sum();
            println!("\nnesting {total} plies on {} for up to {time} s ({} threads, seed {seed}); Ctrl-C stops and keeps the best layout", material.name, opts.workers);
            let cancel = Arc::new(AtomicBool::new(false));
            {
                let c = cancel.clone();
                let _ = ctrlc::set_handler(move || {
                    if c.swap(true, Ordering::SeqCst) {
                        std::process::exit(130);
                    }
                    eprintln!("\nstopping, finishing with the best layout so far (Ctrl-C again to abort)");
                });
            }
            let start = Instant::now();
            let mut best = f64::INFINITY;
            let mut printed = (f64::INFINITY, Duration::ZERO);
            let layout = nest(
                &plies,
                &material,
                &opts,
                |p| {
                    best = best.min(p.length);
                    // Print at most every 2 s, or on a >0.5% improvement.
                    if best < printed.0 * 0.995 || (best < printed.0 && p.elapsed >= printed.1 + Duration::from_secs(2)) {
                        printed = (best, p.elapsed);
                        println!("  [{:>6.1} s {:<8}] length {:>9.1} mm   density {:>5.1}%", p.elapsed.as_secs_f64(), p.phase, best, p.density * 100.0);
                    }
                },
                cancel,
            )?;
            let solve_secs = start.elapsed().as_secs_f64();
            finish(&job.out, &plies, &layout, &warnings, seed, solve_secs)
        }
    }
}

/// Loads job + material, writes the orientation preview.
fn prepare(job: &JobArgs) -> Result<(Vec<PlyShape>, Material, Vec<String>)> {
    let mat_path = job.materials.clone().unwrap_or_else(|| job.job.parent().unwrap_or(Path::new(".")).join("materials.toml"));
    let materials = load_materials(&mat_path)?;
    let Some(material) = materials.iter().find(|m| m.name.eq_ignore_ascii_case(&job.material)).cloned() else {
        let names: Vec<&str> = materials.iter().map(|m| m.name.as_str()).collect();
        bail!("material '{}' not found in {} (available: {})", job.material, mat_path.display(), names.join(", "));
    };
    let rows = read_job_csv(&job.job)?;
    let loaded = load_job(&rows, &job.import.options())?;
    for w in &loaded.warnings {
        eprintln!("warning: {w}");
    }
    std::fs::create_dir_all(&job.out).with_context(|| format!("cannot create {}", job.out.display()))?;
    export::write_orientation_preview(&job.out.join("orientation.svg"), &loaded.plies)?;
    Ok((loaded.plies, material, loaded.warnings))
}

fn print_ply_table(plies: &[PlyShape]) {
    println!("\n{:<22} {:>9} {:>9} {:>8}  {:<13} variants", "shape", "w mm", "h mm", "0° ref", "from");
    for p in plies {
        let b = BBox::of(&p.shape.outer.flatten(0.1, FlattenMode::Inscribed));
        let vs: Vec<String> = p.variants.iter().map(|v| format!("{:+}°×{}", v.angle_deg, v.qty)).collect();
        let flags = [(p.cosmetic, "cosmetic"), (p.shape.mirrored_on_import, "mirrored"), (!p.shape.holes.is_empty(), "holes")]
            .iter()
            .filter(|f| f.0)
            .map(|f| f.1)
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{:<22} {:>9.1} {:>9.1} {:>7.1}°  {:<13} {}{}",
            p.shape.name,
            b.width(),
            b.height(),
            p.ref_deg,
            format!("{:?}", p.ref_origin),
            vs.join(" "),
            if flags.is_empty() { String::new() } else { format!("  [{flags}]") }
        );
    }
}

fn finish(out: &Path, plies: &[PlyShape], layout: &turbonest_core::model::Layout, warnings: &[String], seed: u64, solve_secs: f64) -> Result<i32> {
    let m = &layout.material;
    let report = verify(plies, layout);
    let parts_area: f64 = layout.placements.iter().map(|pl| plies[pl.shape_idx].shape.area(0.01)).sum();
    let roll_area = layout.used_length * m.roll_width;
    let usable_area = (layout.used_length - m.lead_trim).max(0.0) * m.usable_width();
    let summary = serde_json::json!({
        "material": m,
        "used_length_mm": layout.used_length,
        "roll_area_m2": roll_area / 1e6,
        "parts_area_m2": parts_area / 1e6,
        "utilization_of_roll": if roll_area > 0.0 { parts_area / roll_area } else { 0.0 },
        "utilization_of_usable_area": if usable_area > 0.0 { parts_area / usable_area } else { 0.0 },
        "material_cost": m.cost_per_m2.map(|c| c * roll_area / 1e6),
        "plies": layout.placements.len(),
        "solve_seconds": solve_secs,
        "seed": seed,
        "verification": &report,
        "import_warnings": warnings,
        "layout": layout,
    });
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&summary)?)?;
    let title = format!(
        "{} — {} plies — used length {:.1} mm — utilisation {:.1}% of roll{}",
        m.name,
        layout.placements.len(),
        layout.used_length,
        100.0 * parts_area / roll_area.max(1e-9),
        if report.ok { "" } else { " — FAILED VERIFICATION" }
    );
    export::write_svg(&out.join("layout.svg"), plies, layout, &title)?;

    println!("\nused length   {:.1} mm  ({:.3} m² of roll)", layout.used_length, roll_area / 1e6);
    println!("utilisation   {:.1}% of roll, {:.1}% of usable area", 100.0 * parts_area / roll_area.max(1e-9), 100.0 * parts_area / usable_area.max(1e-9));
    if let Some(c) = m.cost_per_m2 {
        println!("material cost {:.2}", c * roll_area / 1e6);
    }
    match report.min_gap {
        Some(g) => println!("smallest gap  {g:.3} mm (spacing {:.3} mm)", m.spacing),
        None => println!("smallest gap  -"),
    }
    println!("fibre error   max {:.4}° (tolerance {:.2}°)", report.max_fibre_deviation, m.angle_tolerance);
    if !report.ok {
        eprintln!("\nVERIFICATION FAILED — no cut files written:");
        for i in &report.issues {
            eprintln!("  - {i}");
        }
        println!("wrote {} and {} for diagnosis", out.join("layout.svg").display(), out.join("report.json").display());
        return Ok(2);
    }
    export::write_dxf(&out.join("layout.dxf"), plies, layout)?;
    export::write_ply_csv(&out.join("plies.csv"), plies, layout)?;
    println!("verification  passed");
    println!("\nwrote {}/{{layout.dxf, layout.svg, plies.csv, report.json, orientation.svg}}", out.display());
    Ok(0)
}

fn inspect(files: &[PathBuf], opts: &ImportOptions) -> Result<i32> {
    if files.is_empty() {
        bail!("give one or more DXF files");
    }
    let mut failed = false;
    for f in files {
        println!("{}", f.display());
        match import_dxf(f, opts) {
            Ok(res) => {
                for s in &res.shapes {
                    let b = BBox::of(&s.outer.flatten(0.1, FlattenMode::Inscribed));
                    let longest = s.outer.straight_segments().into_iter().max_by(|a, b| a.0.total_cmp(&b.0));
                    println!(
                        "  {:<20} {:>8.1} x {:>8.1} mm  area {:>10.1} mm²  holes {}  marks {}  longest edge {}  fibre layer {}",
                        s.name,
                        b.width(),
                        b.height(),
                        s.area(0.01),
                        s.holes.len(),
                        s.marks.len(),
                        longest.map_or("-".into(), |(l, a)| format!("{l:.1} mm @ {a:.1}°")),
                        s.layer_ref_deg.map_or("-".into(), |a| format!("{a:.1}°"))
                    );
                }
                for w in &res.warnings {
                    println!("  warning: {w}");
                }
            }
            Err(e) => {
                failed = true;
                println!("  error: {e:#}");
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}
