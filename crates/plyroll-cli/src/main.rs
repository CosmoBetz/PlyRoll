//! `plyroll` command-line tool.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use plyroll_core::export;
use plyroll_core::geom::{BBox, FlattenMode};
use plyroll_core::import::{ImportOptions, UnitsOverride, import_dxf};
use plyroll_core::job::{load_job, load_materials, read_job_csv};
use plyroll_core::model::{Material, PlyShape};
use plyroll_core::project::{self, Project};
use plyroll_core::report::{ReportMeta, RunInfo, summarize, write_outputs};
use plyroll_nest::{NestOptions, nest};

#[derive(Parser)]
#[command(name = "plyroll", version, about = "Nest prepreg plies on a roll, keeping every ply's fibre direction.")]
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
    /// Job CSV (columns: file, ref, variants, cosmetic, mirror, kit, name) or a .plyroll project.
    job: PathBuf,
    /// Material library (TOML). Default: materials.toml next to the job file.
    #[arg(long)]
    materials: Option<PathBuf>,
    /// Material name from the library. Optional for a .plyroll project that
    /// already has a material.
    #[arg(long, short)]
    material: Option<String>,
    /// Output folder.
    #[arg(long, short, default_value = "out")]
    out: PathBuf,
    #[command(flatten)]
    import: ImportArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// Nest a job and write the cut files (layout.dxf, plies.csv, labels.csv), report.pdf,
    /// layout.svg and report.json.
    Nest {
        #[command(flatten)]
        job: JobArgs,
        /// Optimisation time in seconds (default: the project's time, else 60).
        #[arg(long, short)]
        time: Option<u64>,
        /// Random seed. Runs differ anyway when several threads race against the time limit.
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
    /// Convert a job CSV into a .plyroll project for the orientation editor.
    Convert {
        /// Job CSV.
        job: PathBuf,
        /// Project file to write (default: the CSV name with .plyroll).
        #[arg(long, short)]
        out: Option<PathBuf>,
        #[command(flatten)]
        import: ImportArgs,
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
        Cmd::Convert { job, out, import } => {
            let rows = read_job_csv(&job)?;
            let name = job.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
            let o = import.options();
            let settings = project::ImportSettings { units: o.units, join_tol: o.join_tol, gap_tol: o.gap_tol, curve_tol: o.curve_tol };
            let (built, warnings) = Project::from_job(&name, &rows, settings)?;
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            let out = out.unwrap_or_else(|| job.with_extension(project::EXTENSION));
            built.save(&out)?;
            println!("wrote {} ({} ply shapes)", out.display(), built.plies.len());
            Ok(0)
        }
        Cmd::Check { job } => {
            let (plies, material, _) = prepare(&job)?;
            println!("{} ply shapes, material {} ({:.0} mm roll, {:.0} mm usable)", plies.len(), material.name, material.roll_width, material.usable_width());
            print_ply_table(&plies);
            println!("\nwrote {}", job.out.join("orientation.svg").display());
            Ok(0)
        }
        Cmd::Nest { job, time, seed, workers, early_stop, dump_instance } => {
            let (plies, material, warnings) = prepare(&job)?;
            let time = time.or_else(|| is_project(&job.job).then(|| Project::load(&job.job).map(|p| p.job.time_s).ok()).flatten()).unwrap_or(60);
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
                let groups = plyroll_nest::build_groups(&plies, &material)?;
                let inst = plyroll_nest::build_instance(&plies, &material, &groups, opts.nest_tol);
                let p = job.out.join("solver_instance.json");
                std::fs::write(&p, serde_json::to_string_pretty(&inst)?)?;
                println!("wrote {}", p.display());
            }
            let total: u32 = plies.iter().flat_map(|p| p.variants.iter().map(|v| v.qty)).sum();
            println!("\nnesting {total} plies on {} for up to {time} s ({} threads, seed {seed}); Ctrl-C stops and keeps the best layout", material.name, opts.workers);
            let cancel = Arc::new(AtomicBool::new(false));
            let stopped = cancel.clone();
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
            let run = RunInfo { seed, solve_seconds: start.elapsed().as_secs_f64(), stopped: stopped.load(Ordering::SeqCst) };
            finish(&job.out, &job.job, &plies, &layout, &warnings, &run)
        }
    }
}

/// Material named on the command line, from the library.
fn library_material(job: &JobArgs, name: &str) -> Result<Material> {
    let mat_path = job.materials.clone().unwrap_or_else(|| job.job.parent().unwrap_or(Path::new(".")).join("materials.toml"));
    let materials = load_materials(&mat_path)?;
    let Some(material) = materials.iter().find(|m| m.name.eq_ignore_ascii_case(name)).cloned() else {
        let names: Vec<&str> = materials.iter().map(|m| m.name.as_str()).collect();
        bail!("material '{}' not found in {} (available: {})", name, mat_path.display(), names.join(", "));
    };
    Ok(material)
}

/// Loads job + material, writes the orientation preview.
fn prepare(job: &JobArgs) -> Result<(Vec<PlyShape>, Material, Vec<String>)> {
    let chosen = job.material.as_deref().map(|n| library_material(job, n)).transpose()?;
    let (plies, material, warnings) = if is_project(&job.job) {
        // A project brings its own material and number of sets.
        let proj = Project::load(&job.job)?;
        if chosen.is_none() && proj.job.material.is_none() {
            bail!("project {} has no material yet: pick one in the app, or pass -m <name>", job.job.display());
        }
        let input = proj.nest_input(chosen.as_ref()).with_context(|| format!("project {}", job.job.display()))?;
        if proj.job.sets > 1 {
            println!("{} sets: every quantity is multiplied by {}", proj.job.sets, proj.job.sets);
        }
        (input.plies, input.material, input.warnings)
    } else {
        let Some(material) = chosen else { bail!("--material is required for a job CSV") };
        let rows = read_job_csv(&job.job)?;
        let loaded = load_job(&rows, &job.import.options())?;
        (loaded.plies, material, loaded.warnings)
    };
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    std::fs::create_dir_all(&job.out).with_context(|| format!("cannot create {}", job.out.display()))?;
    export::write_orientation_preview(&job.out.join("orientation.svg"), &plies)?;
    Ok((plies, material, warnings))
}

fn is_project(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(project::EXTENSION))
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

fn finish(out: &Path, job: &Path, plies: &[PlyShape], layout: &plyroll_core::model::Layout, warnings: &[String], run: &RunInfo) -> Result<i32> {
    let m = &layout.material;
    let sum = summarize(plies, layout);
    let name = job.file_stem().map_or_else(|| "job".to_string(), |s| s.to_string_lossy().into_owned());
    let written = write_outputs(out, plies, layout, &sum, run, warnings, &ReportMeta::new(name))?;
    let report = &sum.verification;

    println!("\nused length   {:.1} mm  ({:.3} m² of roll)", layout.used_length, sum.roll_area_m2);
    println!("utilisation   {:.1}% of roll, {:.1}% of usable area", 100.0 * sum.utilization_of_roll, 100.0 * sum.utilization_of_usable_area);
    if let Some(c) = sum.material_cost {
        println!("material cost {c:.2}");
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
    println!("verification  passed");
    let names: Vec<String> = written.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).chain(["orientation.svg".into()]).collect();
    println!("\nwrote {}/{{{}}}", out.display(), names.join(", "));
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
