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
use plyroll_core::report::{OutputOptions, ReportMeta, RunInfo, summarize, today_utc, write_outputs};
use plyroll_core::rolls::{RollInfo, load_rolls, result_key};
use plyroll_core::template::Paper;
use plyroll_core::kits::{self, Grouping};
use plyroll_nest::{NestOptions, nest_job};

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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GroupingArg {
    Global,
    ByKit,
}

impl From<GroupingArg> for Grouping {
    fn from(g: GroupingArg) -> Self {
        match g {
            GroupingArg::Global => Grouping::Global,
            GroupingArg::ByKit => Grouping::ByKit,
        }
    }
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
    /// Split plies that are wider than the roll into pieces, along the fibre,
    /// at every angle that needs it. Splits saved in a project are kept.
    #[arg(long)]
    auto_split: bool,
    #[command(flatten)]
    import: ImportArgs,
}

/// Options shared by `nest` and `export` for the extra outputs.
#[derive(clap::Args, Clone)]
struct OutArgs {
    /// Also write templates.pdf: the layout at true size, tiled over pages.
    /// a4, a3, letter, or roll:<width mm> for a plotter roll. Default: the project's setting.
    #[arg(long, value_parser = parse_paper)]
    templates: Option<Paper>,
    /// Roll from the inventory to cut from (its id); its batch goes on the
    /// report and labels. Default: the project's roll.
    #[arg(long)]
    roll: Option<String>,
    /// Roll inventory (TOML). Default: rolls.toml next to the job file.
    #[arg(long)]
    rolls: Option<PathBuf>,
}

fn parse_paper(s: &str) -> Result<Paper, String> {
    let p = match s.to_ascii_lowercase().as_str() {
        "a4" => Paper::A4,
        "a3" => Paper::A3,
        "letter" => Paper::Letter,
        other => match other.strip_prefix("roll:") {
            Some(w) => Paper::Roll { width_mm: w.trim().parse().map_err(|_| format!("'{w}' is not a width in mm"))? },
            None => return Err("use a4, a3, letter or roll:<width mm>".into()),
        },
    };
    p.validate().map_err(|e| e.to_string())?;
    Ok(p)
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
        /// global: all plies mixed (shortest roll). by-kit: each kit on its own
        /// section of roll, one after another. Default: the project's setting, else global.
        #[arg(long, value_enum)]
        grouping: Option<GroupingArg>,
        #[command(flatten)]
        extra: OutArgs,
    },
    /// Write the files of the nest saved in a .plyroll project, without nesting again.
    Export {
        /// The project.
        project: PathBuf,
        /// Output folder.
        #[arg(long, short, default_value = "out")]
        out: PathBuf,
        #[command(flatten)]
        extra: OutArgs,
    },
    /// List the rolls in the inventory.
    Rolls {
        /// Roll inventory (TOML).
        #[arg(default_value = "rolls.toml")]
        file: PathBuf,
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
        Cmd::Rolls { file } => {
            let rolls = load_rolls(&file)?;
            let today = today_utc();
            println!("{:<12} {:<22} {:<14} {:>10}  {:<12} booked", "roll", "material", "batch", "left m", "use by");
            for r in &rolls {
                let used: f64 = r.usage.iter().map(|u| u.length_m).fold(0.0, |a, b| a + b);
                let booked = match r.usage.len() {
                    0 => "-".to_string(),
                    n => format!("{used:.2} m in {n} job{}", if n == 1 { "" } else { "s" }),
                };
                println!(
                    "{:<12} {:<22} {:<14} {:>10.2}  {:<12} {booked}{}",
                    r.id,
                    r.material,
                    if r.batch.is_empty() { "-" } else { &r.batch },
                    r.remaining_m,
                    r.expires.as_deref().unwrap_or("-"),
                    if r.expired(&today) { "  EXPIRED" } else { "" }
                );
            }
            Ok(0)
        }
        Cmd::Export { project, out, extra } => {
            let proj = Project::load(&project)?;
            let Some(r) = &proj.result else { bail!("{} has no saved nest: nest it in the app or with `plyroll nest`", project.display()) };
            let f = proj.check_result(r);
            if !f.fresh {
                bail!("the saved nest is out of date ({}). Nest again before exporting.", f.reason.unwrap_or_default());
            }
            let (meta, opts) = out_options(&project, &extra, Some(&proj.job), &r.layout.material, &result_key(&r.fingerprint, r.run.seed))?;
            finish(&out, &r.plies, &r.layout, &r.warnings, &r.run, meta, &opts)
        }
        Cmd::Nest { job, time, seed, workers, early_stop, dump_instance, grouping, extra } => {
            let (plies, material, warnings) = prepare(&job)?;
            let saved = is_project(&job.job).then(|| Project::load(&job.job).ok().map(|p| p.job)).flatten();
            let time = time.or(saved.as_ref().map(|j| j.time_s)).unwrap_or(60);
            let grouping = grouping.map(Grouping::from).or(saved.as_ref().map(|j| j.grouping)).unwrap_or_default();
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
            if grouping == Grouping::ByKit {
                let kits: Vec<String> = kits::kit_order(&plies).into_iter().map(|k| k.unwrap_or_else(|| "(no kit)".into())).collect();
                println!("kit by kit, in this order along the roll: {}", kits.join(", "));
            }
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
            let mut section = usize::MAX;
            let layout = nest_job(
                &plies,
                &material,
                grouping,
                &opts,
                |p| {
                    if let Some(s) = &p.section
                        && s.index != section
                    {
                        section = s.index;
                        best = f64::INFINITY;
                        printed = (f64::INFINITY, Duration::ZERO);
                        println!("  kit {} ({} of {})", s.kit.as_deref().unwrap_or("(no kit)"), s.index + 1, s.count);
                    }
                    best = best.min(p.length);
                    // Print at most every 2 s, or on a >0.5% improvement.
                    if best < printed.0 * 0.995 || (best < printed.0 && p.elapsed >= printed.1 + Duration::from_secs(2)) {
                        printed = (best, p.elapsed);
                        println!("  [{:>6.1} s {:<8}] length {:>9.1} mm   density {:>5.1}%", p.elapsed.as_secs_f64(), p.phase, best, p.density * 100.0);
                    }
                },
                cancel,
            )?;
            let run = RunInfo { seed, solve_seconds: start.elapsed().as_secs_f64(), stopped: stopped.load(Ordering::SeqCst), edits: 0, kept: 0 };
            let (meta, opts) = out_options(&job.job, &extra, saved.as_ref(), &material, "")?;
            finish(&job.out, &plies, &layout, &warnings, &run, meta, &opts)
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
        let mut proj = Project::load(&job.job)?;
        if job.auto_split {
            let n = proj.auto_split(chosen.as_ref())?;
            if n > 0 {
                println!("split {n} angle{} wider than the roll", if n == 1 { "" } else { "s" });
            }
        }
        if chosen.is_none() && proj.job.material.is_none() {
            bail!("project {} has no material yet: pick one in the app, or pass -m <name>", job.job.display());
        }
        let input = proj.nest_input(chosen.as_ref()).map_err(|e| {
            let msg = format!("project {}: {e:#}", job.job.display());
            if !job.auto_split && msg.contains("wider than the roll") { anyhow::anyhow!("{msg}\nor pass --auto-split") } else { anyhow::anyhow!(msg) }
        })?;
        if proj.job.sets > 1 {
            println!("{} sets: every quantity is multiplied by {}", proj.job.sets, proj.job.sets);
        }
        (input.plies, input.material, input.warnings)
    } else {
        let Some(material) = chosen else { bail!("--material is required for a job CSV") };
        let rows = read_job_csv(&job.job)?;
        let mut loaded = load_job(&rows, &job.import.options())?;
        if job.auto_split {
            let n = plyroll_core::split::auto_split_plies(&mut loaded.plies, &material);
            if n > 0 {
                println!("split {n} angle{} wider than the roll", if n == 1 { "" } else { "s" });
            }
        }
        let ids = vec![0; loaded.plies.len()];
        let (plies, _) = plyroll_core::split::expand(&loaded.plies, &ids, &material)?;
        let wide = plyroll_core::split::too_wide(&plies, &material);
        if !wide.is_empty() {
            bail!("wider than the roll ({:.0} mm usable); pass --auto-split to split them:\n  {}", material.usable_width(), wide.join("\n  "));
        }
        (plies, material, loaded.warnings)
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

/// Report details and optional outputs from the command line and the project.
/// `key` identifies a saved nest (empty for a new one), so a booking of it on
/// the roll is not counted twice.
fn out_options(job: &Path, extra: &OutArgs, saved: Option<&project::JobSettings>, material: &Material, key: &str) -> Result<(ReportMeta, OutputOptions)> {
    let name = job.file_stem().map_or_else(|| "job".to_string(), |s| s.to_string_lossy().into_owned());
    let templates = extra.templates.or(saved.and_then(|j| j.templates));
    let explicit = extra.roll.is_some();
    let Some(id) = extra.roll.clone().or(saved.and_then(|j| j.roll.clone())) else {
        return Ok((ReportMeta::new(name), OutputOptions { templates }));
    };
    let path = extra.rolls.clone().unwrap_or_else(|| job.parent().unwrap_or(Path::new(".")).join("rolls.toml"));
    // A roll the project picked but this machine has no inventory for is only a warning.
    let soft = |e: anyhow::Error| -> Result<(ReportMeta, OutputOptions)> {
        if explicit {
            return Err(e);
        }
        eprintln!("warning: {e:#}; the report has no roll");
        Ok((ReportMeta::new(name.clone()), OutputOptions { templates }))
    };
    let rolls = match load_rolls(&path) {
        Ok(r) => r,
        Err(e) => return soft(e.context(format!("roll '{id}' needs the inventory"))),
    };
    let Some(roll) = rolls.iter().find(|r| r.id.eq_ignore_ascii_case(&id)) else {
        return soft(anyhow::anyhow!("roll '{id}' is not in {}", path.display()));
    };
    if !roll.material.eq_ignore_ascii_case(&material.name) {
        return soft(anyhow::anyhow!("roll '{}' is {}, but the job is nested on {}", roll.id, roll.material, material.name));
    }
    let today = today_utc();
    let info = RollInfo::of(roll, key, &today);
    if roll.expired(&today) {
        eprintln!("warning: roll '{}' expired on {}", roll.id, roll.expires.as_deref().unwrap_or_default());
    }
    Ok((ReportMeta::new(name).with_roll(Some(info)), OutputOptions { templates }))
}

fn finish(out: &Path, plies: &[PlyShape], layout: &plyroll_core::model::Layout, warnings: &[String], run: &RunInfo, meta: ReportMeta, opts: &OutputOptions) -> Result<i32> {
    let m = &layout.material;
    let sum = summarize(plies, layout);
    std::fs::create_dir_all(out).with_context(|| format!("cannot create {}", out.display()))?;
    let written = write_outputs(out, plies, layout, &sum, run, warnings, &meta, opts)?;
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
    if let Some(r) = &meta.roll {
        let need = layout.used_length / 1000.0;
        if need > r.before_m + 1e-9 {
            eprintln!("warning: NOT ENOUGH ROLL: the job needs {need:.2} m, roll '{}' has {:.2} m", r.id, r.before_m);
        } else {
            println!("roll          {} (batch {}): {:.2} m before, {:.2} m after", r.id, if r.batch.is_empty() { "-" } else { &r.batch }, r.before_m, r.before_m - need);
        }
    }
    for s in &layout.sections {
        println!("  kit {:<12} {:>9.1} .. {:>9.1} mm", s.kit.as_deref().unwrap_or("(no kit)"), s.start, s.end);
    }
    let names: Vec<String> = written.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
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
