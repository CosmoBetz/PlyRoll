//! PlyRoll desktop app. The project lives here in Rust; the web UI calls the
//! commands below and redraws from the returned view after every change.
//!
//! Nesting runs on its own thread and reports through window events
//! (`nest-progress`, `nest-done`). The last result is kept next to the
//! project, not inside it, so undo never throws a long nest away; it is
//! written into the project file on save.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use plyroll_core::job::{load_materials, save_materials};
use plyroll_core::model::{Material, Variant};
use plyroll_core::project::{ImportReport, ImportSettings, JobSettings, NestResult, Project, RefPick, SourceFile};
use plyroll_core::report::{ReportMeta, RunInfo, write_outputs};
use plyroll_core::view::{LayoutShapes, PlacementView, PlyView, ResultView, layout_shapes, placement_views, project_view, result_view};
use plyroll_nest::{NestOptions, nest};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

const UNDO_LIMIT: usize = 200;
/// How often the running nest sends its current layout to the window.
const LIVE_EVERY: Duration = Duration::from_millis(400);
/// Run ids, unique for the app's lifetime (New / Open replace the state).
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

struct Run {
    id: u64,
    cancel: Arc<AtomicBool>,
}

struct AppState {
    /// `project.result` is always None here; see `result`.
    project: Project,
    result: Option<NestResult>,
    path: Option<PathBuf>,
    dirty: bool,
    undo: Vec<Project>,
    redo: Vec<Project>,
    run: Option<Run>,
}

impl AppState {
    fn with(mut project: Project, path: Option<PathBuf>) -> Self {
        let result = project.result.take();
        AppState { project, result, path, dirty: false, undo: Vec::new(), redo: Vec::new(), run: None }
    }

    fn fresh() -> Self {
        Self::with(Project::new("Untitled"), None)
    }

    /// Runs a change as one undo step. Nothing is recorded when it fails.
    fn change<T>(&mut self, f: impl FnOnce(&mut Project) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let before = self.project.clone();
        match f(&mut self.project) {
            Ok(v) => {
                // A no-op (e.g. re-importing an unchanged DXF) must not eat the redo stack.
                if same(&before, &self.project) {
                    return Ok(v);
                }
                self.undo.push(before);
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.dirty = true;
                Ok(v)
            }
            Err(e) => {
                self.project = before;
                Err(e)
            }
        }
    }

    /// Stops a running nest; its result is then dropped.
    fn cancel_run(&mut self) {
        if let Some(r) = self.run.take() {
            r.cancel.store(true, Ordering::SeqCst);
        }
    }
}

fn same(a: &Project, b: &Project) -> bool {
    matches!((serde_json::to_value(a), serde_json::to_value(b)), (Ok(x), Ok(y)) if x == y)
}

/// The material library, a TOML file in the app's config folder (the same
/// format the CLI reads with `--materials`).
struct Library {
    path: PathBuf,
    materials: Vec<Material>,
    /// The file exists but could not be read; saving is refused so it is not overwritten.
    error: Option<String>,
}

impl Library {
    fn load(path: PathBuf) -> Self {
        if !path.exists() {
            return Library { path, materials: Vec::new(), error: None };
        }
        match load_materials(&path) {
            Ok(materials) => Library { path, materials, error: None },
            Err(e) => Library { path, materials: Vec::new(), error: Some(format!("{e:#}")) },
        }
    }

    fn save(&self, materials: Vec<Material>) -> anyhow::Result<Vec<Material>> {
        if let Some(e) = &self.error {
            anyhow::bail!("the material library could not be read, so it is not overwritten. Fix or remove {}.\n{e}", self.path.display());
        }
        save_materials(&self.path, &materials)?;
        Ok(materials)
    }
}

#[derive(Serialize)]
struct LibraryView {
    path: String,
    materials: Vec<Material>,
    error: Option<String>,
}

fn library_view(lib: &Library) -> LibraryView {
    LibraryView { path: lib.path.display().to_string(), materials: lib.materials.clone(), error: lib.error.clone() }
}

#[derive(Serialize)]
struct Readiness {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
struct AppView {
    name: String,
    path: Option<String>,
    dirty: bool,
    can_undo: bool,
    can_redo: bool,
    plies: Vec<PlyView>,
    ready: Readiness,
    job: JobSettings,
    import: ImportSettings,
    /// Can nesting start (plies ready and a material chosen)?
    nest_ready: Readiness,
    result: Option<ResultView>,
    /// Id of the nest that is running, if any.
    running: Option<u64>,
}

#[derive(Serialize)]
struct ImportResult {
    view: AppView,
    report: ImportReport,
}

type Shared<'a> = State<'a, Mutex<AppState>>;
type SharedLib<'a> = State<'a, Mutex<Library>>;
type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn total(plies: &[plyroll_core::model::PlyShape]) -> u32 {
    plies.iter().flat_map(|p| p.variants.iter().map(|v| v.qty)).sum()
}

fn view(st: &AppState, window: &WebviewWindow) -> AppView {
    let ready = match st.project.to_plies() {
        Ok((plies, warnings)) => {
            let mut message = format!("{} plies from {} shapes ready to nest", total(&plies), plies.len());
            if !warnings.is_empty() {
                message.push_str(&format!(" ({} without quantities left out)", warnings.len()));
            }
            Readiness { ok: true, message }
        }
        Err(e) => Readiness { ok: false, message: format!("{e:#}") },
    };
    let nest_ready = match st.project.nest_input(None) {
        Ok(input) => Readiness { ok: true, message: format!("{} plies on {}", total(&input.plies), input.material.name) },
        Err(e) => Readiness { ok: false, message: format!("{e:#}") },
    };
    let file = st.path.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned());
    let title = format!("{}{} — PlyRoll", if st.dirty { "• " } else { "" }, file.as_deref().unwrap_or("Untitled"));
    let _ = window.set_title(&title);
    AppView {
        name: st.project.name.clone(),
        path: st.path.as_ref().map(|p| p.display().to_string()),
        dirty: st.dirty,
        can_undo: !st.undo.is_empty(),
        can_redo: !st.redo.is_empty(),
        plies: project_view(&st.project),
        ready,
        job: st.project.job.clone(),
        import: st.project.import.clone(),
        nest_ready,
        result: st.result.as_ref().map(|r| result_view(&st.project, r)),
        running: st.run.as_ref().map(|r| r.id),
    }
}

/// Applies an undoable change and returns the new view.
fn edit<T>(state: &Shared, window: &WebviewWindow, f: impl FnOnce(&mut Project) -> anyhow::Result<T>) -> CmdResult<AppView> {
    let mut st = state.lock().unwrap();
    st.change(f).map_err(err)?;
    Ok(view(&st, window))
}

#[tauri::command]
fn get_state(state: Shared, window: WebviewWindow) -> AppView {
    view(&state.lock().unwrap(), &window)
}

#[tauri::command]
fn new_project(state: Shared, window: WebviewWindow) -> AppView {
    let mut st = state.lock().unwrap();
    st.cancel_run();
    *st = AppState::fresh();
    view(&st, &window)
}

#[tauri::command]
fn open_project(path: String, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    let path = PathBuf::from(path);
    let project = Project::load(&path).map_err(err)?;
    let mut st = state.lock().unwrap();
    st.cancel_run();
    *st = AppState::with(project, Some(path));
    Ok(view(&st, &window))
}

/// Saves to `path`, or to the current file when `path` is empty.
#[tauri::command]
fn save_project(path: Option<String>, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    let mut st = state.lock().unwrap();
    let mut target = match path {
        Some(p) => PathBuf::from(p),
        None => st.path.clone().ok_or("choose where to save the project")?,
    };
    if target.extension().is_none() {
        target.set_extension(plyroll_core::project::EXTENSION);
    }
    if st.project.name == "Untitled" {
        if let Some(stem) = target.file_stem() {
            st.project.name = stem.to_string_lossy().into_owned();
        }
    }
    // The result travels in the file but stays out of the undo snapshots.
    st.project.result = st.result.take();
    let saved = st.project.save(&target);
    st.result = st.project.result.take();
    saved.map_err(err)?;
    st.path = Some(target);
    st.dirty = false;
    Ok(view(&st, &window))
}

#[tauri::command]
fn import_dxf(paths: Vec<String>, state: Shared, window: WebviewWindow) -> CmdResult<ImportResult> {
    let files: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let mut st = state.lock().unwrap();
    let report = st.change(|p| p.import_files(&files)).map_err(err)?;
    Ok(ImportResult { view: view(&st, &window), report })
}

#[tauri::command]
fn reload_sources(state: Shared, window: WebviewWindow) -> CmdResult<ImportResult> {
    let mut st = state.lock().unwrap();
    let report = st.change(|p| p.reload()).map_err(err)?;
    Ok(ImportResult { view: view(&st, &window), report })
}

/// Changes units and tolerances and re-reads the DXFs with them (one undo step).
#[tauri::command]
fn set_import(settings: ImportSettings, state: Shared, window: WebviewWindow) -> CmdResult<ImportResult> {
    let mut st = state.lock().unwrap();
    let report = st.change(|p| p.set_import(settings)).map_err(err)?;
    Ok(ImportResult { view: view(&st, &window), report })
}

/// The project's DXFs with the units each declares (reads the files).
#[tauri::command]
fn import_sources(state: Shared) -> Vec<SourceFile> {
    let project = state.lock().unwrap().project.clone();
    project.sources()
}

#[tauri::command]
fn set_reference(ids: Vec<u64>, pick: RefPick, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_reference(&ids, &pick))
}

#[tauri::command]
fn set_variants(ids: Vec<u64>, variants: Vec<Variant>, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_variants(&ids, &variants))
}

#[tauri::command]
fn set_cosmetic(ids: Vec<u64>, on: bool, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_cosmetic(&ids, on))
}

#[tauri::command]
fn set_mirrored(ids: Vec<u64>, on: bool, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_mirrored(&ids, on))
}

#[tauri::command]
fn rename_ply(id: u64, name: String, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.rename(id, &name))
}

#[tauri::command]
fn remove_plies(ids: Vec<u64>, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| {
        p.remove(&ids);
        Ok(())
    })
}

#[tauri::command]
fn set_job(job: JobSettings, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_job(job))
}

#[tauri::command]
fn undo(state: Shared, window: WebviewWindow) -> AppView {
    let mut st = state.lock().unwrap();
    if let Some(prev) = st.undo.pop() {
        let cur = std::mem::replace(&mut st.project, prev);
        st.redo.push(cur);
        st.dirty = true;
    }
    view(&st, &window)
}

#[tauri::command]
fn redo(state: Shared, window: WebviewWindow) -> AppView {
    let mut st = state.lock().unwrap();
    if let Some(next) = st.redo.pop() {
        let cur = std::mem::replace(&mut st.project, next);
        st.undo.push(cur);
        st.dirty = true;
    }
    view(&st, &window)
}

// ---- material library -------------------------------------------------------

#[tauri::command]
fn get_library(lib: SharedLib) -> LibraryView {
    library_view(&lib.lock().unwrap())
}

/// Adds or replaces a material. `original` is its name before an edit.
#[tauri::command]
fn save_material(original: Option<String>, material: Material, lib: SharedLib) -> CmdResult<LibraryView> {
    let mut lib = lib.lock().unwrap();
    let mut list = lib.materials.clone();
    let name = material.name.trim().to_string();
    let material = Material { name: name.clone(), ..material };
    let pos = original.as_deref().and_then(|o| list.iter().position(|m| m.name == o));
    if list.iter().enumerate().any(|(i, m)| Some(i) != pos && m.name.eq_ignore_ascii_case(&name)) {
        return Err(format!("a material named '{name}' already exists"));
    }
    match pos {
        Some(i) => list[i] = material,
        None => list.push(material),
    }
    lib.materials = lib.save(list).map_err(err)?;
    Ok(library_view(&lib))
}

#[tauri::command]
fn delete_material(name: String, lib: SharedLib) -> CmdResult<LibraryView> {
    let mut lib = lib.lock().unwrap();
    let list: Vec<Material> = lib.materials.iter().filter(|m| m.name != name).cloned().collect();
    lib.materials = lib.save(list).map_err(err)?;
    Ok(library_view(&lib))
}

/// Merges a TOML material file (CLI format) into the library; same names are replaced.
#[tauri::command]
fn import_materials(path: String, lib: SharedLib) -> CmdResult<LibraryView> {
    let incoming = load_materials(&PathBuf::from(path)).map_err(err)?;
    let mut lib = lib.lock().unwrap();
    let mut list = lib.materials.clone();
    for m in incoming {
        match list.iter().position(|x| x.name.eq_ignore_ascii_case(&m.name)) {
            Some(i) => list[i] = m,
            None => list.push(m),
        }
    }
    lib.materials = lib.save(list).map_err(err)?;
    Ok(library_view(&lib))
}

// ---- nesting ----------------------------------------------------------------

#[derive(Clone, Serialize)]
struct NestStarted {
    run: u64,
    time_s: u64,
    shapes: LayoutShapes,
}

#[derive(Clone, Serialize)]
struct NestProgress {
    run: u64,
    phase: &'static str,
    length: f64,
    density: f64,
    elapsed: f64,
    placements: Option<Vec<PlacementView>>,
}

#[derive(Clone, Serialize)]
struct NestDone {
    run: u64,
    error: Option<String>,
    stopped: bool,
}

#[tauri::command]
fn start_nest(app: AppHandle, state: Shared) -> CmdResult<NestStarted> {
    let mut st = state.lock().unwrap();
    if st.run.is_some() {
        return Err("a nest is already running".into());
    }
    let input = st.project.nest_input(None).map_err(err)?;
    // Fail fast (e.g. a ply wider than the roll) before starting the thread.
    plyroll_nest::build_groups(&input.plies, &input.material).map_err(err)?;
    let id = RUN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    st.run = Some(Run { id, cancel: cancel.clone() });
    let time_s = st.project.job.time_s;
    let started = NestStarted { run: id, time_s, shapes: layout_shapes(&input.plies, &input.material) };
    drop(st);

    std::thread::spawn(move || {
        // Seeds stay below 2^53 so they survive JSON in the web view exactly.
        let seed = now_nanos() & ((1 << 53) - 1);
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1);
        let opts = NestOptions { time: Duration::from_secs(time_s), seed: Some(seed), workers, live_layout: Some(LIVE_EVERY), ..NestOptions::default() };
        let start = Instant::now();
        let mut best = f64::INFINITY;
        let mut last_emit: Option<Instant> = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            nest(
                &input.plies,
                &input.material,
                &opts,
                |p| {
                    let improved = p.length < best - 1e-6;
                    best = best.min(p.length);
                    let due = last_emit.is_none_or(|t| t.elapsed() >= Duration::from_millis(150));
                    if p.layout.is_some() || (improved && due) {
                        last_emit = Some(Instant::now());
                        let placements = p.layout.as_ref().map(|l| placement_views(&input.plies, l));
                        let _ = app.emit(
                            "nest-progress",
                            NestProgress { run: id, phase: p.phase, length: best, density: p.density, elapsed: p.elapsed.as_secs_f64(), placements },
                        );
                    }
                },
                cancel.clone(),
            )
        }));
        let stopped = cancel.load(Ordering::SeqCst);
        let state = app.state::<Mutex<AppState>>();
        let mut st = state.lock().unwrap();
        if st.run.as_ref().map(|r| r.id) != Some(id) {
            return; // superseded by New / Open
        }
        st.run = None;
        let error = match outcome {
            Ok(Ok(layout)) => {
                let run = RunInfo { seed, solve_seconds: start.elapsed().as_secs_f64(), stopped };
                st.result = Some(NestResult::new(input, layout, run));
                st.dirty = true;
                None
            }
            Ok(Err(e)) => Some(format!("{e:#}")),
            Err(panic) => {
                let msg = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
                Some(format!("the solver crashed: {}", msg.unwrap_or_else(|| "unknown error".into())))
            }
        };
        drop(st);
        let _ = app.emit("nest-done", NestDone { run: id, error, stopped });
    });
    Ok(started)
}

#[tauri::command]
fn stop_nest(state: Shared) {
    if let Some(r) = &state.lock().unwrap().run {
        r.cancel.store(true, Ordering::SeqCst);
    }
}

/// Writes the cut files of the current result into `dir`.
#[tauri::command]
fn export_result(dir: String, state: Shared) -> CmdResult<Vec<String>> {
    let st = state.lock().unwrap();
    let r = st.result.as_ref().ok_or("nothing nested yet")?;
    let f = st.project.check_result(r);
    if !f.fresh {
        return Err(format!("the nest is out of date ({}). Nest again before exporting.", f.reason.unwrap_or_default()));
    }
    if !r.summary.verification.ok {
        return Err(format!("the nest failed verification, so no cut files are written:\n{}", r.summary.verification.issues.join("\n")));
    }
    let job = st.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
    let files = write_outputs(&PathBuf::from(dir), &r.plies, &r.layout, &r.summary, &r.run, &r.warnings, &ReportMeta::new(job)).map_err(err)?;
    Ok(files.iter().map(|p| p.display().to_string()).collect())
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Mutex::new(AppState::fresh()));
            let lib_path = app.path().app_config_dir().map(|d| d.join("materials.toml")).unwrap_or_else(|_| PathBuf::from("materials.toml"));
            app.manage(Mutex::new(Library::load(lib_path)));
            // `plyroll-app path/to/project.plyroll` opens it on start.
            if let Some(arg) = std::env::args().nth(1).filter(|a| a.ends_with(".plyroll")) {
                let path = PathBuf::from(&arg);
                match Project::load(&path) {
                    Ok(project) => {
                        let st = app.state::<Mutex<AppState>>();
                        *st.lock().unwrap() = AppState::with(project, Some(path));
                    }
                    Err(e) => eprintln!("cannot open {arg}: {e:#}"),
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            new_project,
            open_project,
            save_project,
            import_dxf,
            reload_sources,
            set_import,
            import_sources,
            set_reference,
            set_variants,
            set_cosmetic,
            set_mirrored,
            rename_ply,
            remove_plies,
            set_job,
            undo,
            redo,
            get_library,
            save_material,
            delete_material,
            import_materials,
            start_nest,
            stop_nest,
            export_result,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PlyRoll");
}
