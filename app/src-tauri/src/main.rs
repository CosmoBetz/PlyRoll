//! PlyRoll desktop app. The project lives here in Rust; the web UI calls the
//! commands below and redraws from the returned view after every change.
//!
//! Nesting runs on its own thread and reports through window events
//! (`nest-progress`, `nest-done`). The last result is kept next to the
//! project, not inside it, so undo never throws a long nest away; it is
//! written into the project file on save.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use plyroll_core::edit::{Edit, EditSnapshot};
use plyroll_core::job::{load_materials, save_materials};
use plyroll_core::model::{Material, Variant};
use plyroll_core::project::{ImportReport, ImportSettings, JobSettings, NestResult, Project, RefPick, SourceFile};
use plyroll_core::report::{OutputOptions, ReportMeta, RunInfo, now_utc, today_utc, write_outputs};
use plyroll_core::rolls::{Roll, RollInfo, Usage, load_rolls, result_key, save_rolls};
use plyroll_core::view::{LayoutShapes, PlacementView, PlyView, ResultView, layout_shapes, placement_views, project_view, result_view};
use plyroll_core::kits::LayoutSection;
use plyroll_nest::{NestOptions, Progress, nest_after_locked, nest_job};
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
    /// Hand edits of `result`, apart from the project's undo; cleared by a new nest.
    edit_undo: Vec<EditSnapshot>,
    edit_redo: Vec<EditSnapshot>,
    run: Option<Run>,
}

impl AppState {
    fn with(mut project: Project, path: Option<PathBuf>) -> Self {
        let result = project.result.take();
        AppState { project, result, path, dirty: false, undo: Vec::new(), redo: Vec::new(), edit_undo: Vec::new(), edit_redo: Vec::new(), run: None }
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

/// The roll inventory, `rolls.toml` next to the material library.
struct Inventory {
    path: PathBuf,
    rolls: Vec<Roll>,
    /// The file exists but could not be read; saving is refused so it is not overwritten.
    error: Option<String>,
}

impl Inventory {
    fn load(path: PathBuf) -> Self {
        if !path.exists() {
            return Inventory { path, rolls: Vec::new(), error: None };
        }
        match load_rolls(&path) {
            Ok(rolls) => Inventory { path, rolls, error: None },
            Err(e) => Inventory { path, rolls: Vec::new(), error: Some(format!("{e:#}")) },
        }
    }

    /// Saves a changed list; the inventory is only replaced when the file was written.
    fn save(&mut self, rolls: Vec<Roll>) -> anyhow::Result<()> {
        if let Some(e) = &self.error {
            anyhow::bail!("the roll inventory could not be read, so it is not overwritten. Fix or remove {}.\n{e}", self.path.display());
        }
        save_rolls(&self.path, &rolls)?;
        self.rolls = rolls;
        Ok(())
    }

    fn find(&self, id: &str) -> Option<&Roll> {
        self.rolls.iter().find(|r| r.id == id)
    }
}

#[derive(Serialize)]
struct RollsView {
    path: String,
    rolls: Vec<Roll>,
    error: Option<String>,
    /// Today, YYYY-MM-DD, for expiry.
    today: String,
}

fn rolls_view(inv: &Inventory) -> RollsView {
    RollsView { path: inv.path.display().to_string(), rolls: inv.rolls.clone(), error: inv.error.clone(), today: today_utc() }
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
    can_undo_edit: bool,
    can_redo_edit: bool,
    plies: Vec<PlyView>,
    ready: Readiness,
    job: JobSettings,
    import: ImportSettings,
    /// Can nesting start (plies ready and a material chosen)?
    nest_ready: Readiness,
    result: Option<ResultView>,
    /// Id of the nest that is running, if any.
    running: Option<u64>,
    /// Angles wider than the roll that have no split yet.
    too_wide: usize,
}

#[derive(Serialize)]
struct ImportResult {
    view: AppView,
    report: ImportReport,
}

type Shared<'a> = State<'a, Mutex<AppState>>;
type SharedLib<'a> = State<'a, Mutex<Library>>;
type SharedInv<'a> = State<'a, Mutex<Inventory>>;
type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn total(plies: &[plyroll_core::model::PlyShape]) -> u32 {
    plies.iter().flat_map(|p| p.variants.iter().map(|v| v.qty)).sum()
}

fn view(st: &AppState, window: &WebviewWindow) -> AppView {
    let mut unsplit = 0;
    let ready = match st.project.to_plies() {
        Ok((plies, warnings)) => {
            unsplit = total(&plies) * st.project.job.sets.max(1);
            let mut message = format!("{} plies from {} shapes ready to nest", total(&plies), plies.len());
            if !warnings.is_empty() {
                message.push_str(&format!(" ({} without quantities left out)", warnings.len()));
            }
            Readiness { ok: true, message }
        }
        Err(e) => Readiness { ok: false, message: format!("{e:#}") },
    };
    let nest_ready = match st.project.nest_input(None) {
        Ok(input) => {
            let n = total(&input.plies);
            let before = if n != unsplit { format!(" ({unsplit} before splitting)") } else { String::new() };
            Readiness { ok: true, message: format!("{n} plies on {}{before}", input.material.name) }
        }
        Err(e) => Readiness { ok: false, message: format!("{e:#}") },
    };
    let plies = project_view(&st.project);
    let file = st.path.as_ref().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned());
    let title = format!("{}{} — PlyRoll", if st.dirty { "• " } else { "" }, file.as_deref().unwrap_or("Untitled"));
    let _ = window.set_title(&title);
    AppView {
        name: st.project.name.clone(),
        path: st.path.as_ref().map(|p| p.display().to_string()),
        dirty: st.dirty,
        can_undo: !st.undo.is_empty(),
        can_redo: !st.redo.is_empty(),
        can_undo_edit: !st.edit_undo.is_empty(),
        can_redo_edit: !st.edit_redo.is_empty(),
        too_wide: plyroll_core::view::too_wide_count(&plies),
        plies,
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
fn set_split(id: u64, variant: usize, split: Option<plyroll_core::split::Split>, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| p.set_split(id, variant, split))
}

#[tauri::command]
fn auto_split(state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    edit(&state, &window, |p| match p.auto_split(None)? {
        0 => anyhow::bail!("no angle is wider than the roll"),
        _ => Ok(()),
    })
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
fn save_material(original: Option<String>, material: Material, lib: SharedLib, inv: SharedInv) -> CmdResult<LibraryView> {
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
    // A renamed material keeps its rolls.
    if let Some(old) = original.filter(|o| *o != name) {
        let mut inv = inv.lock().unwrap();
        if inv.rolls.iter().any(|r| r.material == old) {
            let rolls = inv.rolls.iter().cloned().map(|r| if r.material == old { Roll { material: name.clone(), ..r } } else { r }).collect();
            if let Err(e) = inv.save(rolls) {
                return Err(format!("the material was renamed, but its rolls still name '{old}': {}", err(e)));
            }
        }
    }
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

// ---- roll inventory ---------------------------------------------------------

#[tauri::command]
fn get_rolls(inv: SharedInv) -> RollsView {
    rolls_view(&inv.lock().unwrap())
}

/// Adds or replaces a roll. `original` is its id before an edit. Its usage
/// history is kept from the inventory, never taken from the UI.
#[tauri::command]
fn save_roll(original: Option<String>, roll: Roll, inv: SharedInv) -> CmdResult<RollsView> {
    let mut inv = inv.lock().unwrap();
    let mut list = inv.rolls.clone();
    let roll = Roll { id: roll.id.trim().to_string(), material: roll.material.trim().to_string(), batch: roll.batch.trim().to_string(), ..roll };
    let pos = original.as_deref().and_then(|o| list.iter().position(|r| r.id == o));
    if list.iter().enumerate().any(|(i, r)| Some(i) != pos && r.id.eq_ignore_ascii_case(&roll.id)) {
        return Err(format!("a roll with the id '{}' already exists", roll.id));
    }
    match pos {
        Some(i) => {
            let usage = std::mem::take(&mut list[i].usage);
            list[i] = Roll { usage, ..roll };
        }
        None => list.push(Roll { usage: Vec::new(), ..roll }),
    }
    inv.save(list).map_err(err)?;
    Ok(rolls_view(&inv))
}

#[tauri::command]
fn delete_roll(id: String, inv: SharedInv) -> CmdResult<RollsView> {
    let mut inv = inv.lock().unwrap();
    let list = inv.rolls.iter().filter(|r| r.id != id).cloned().collect();
    inv.save(list).map_err(err)?;
    Ok(rolls_view(&inv))
}

/// The job's roll, checked against the result's material.
fn job_roll<'a>(st: &AppState, inv: &'a Inventory, material: &str) -> CmdResult<Option<&'a Roll>> {
    let Some(id) = &st.project.job.roll else { return Ok(None) };
    let roll = inv.find(id).ok_or_else(|| format!("roll '{id}' is not in the inventory. Pick another roll, or none."))?;
    if roll.material != material {
        return Err(format!("roll '{id}' is {}, but the nest is on {material}. Pick a roll of {material}.", roll.material));
    }
    Ok(Some(roll))
}

fn job_name(st: &AppState) -> String {
    st.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Takes the current nest's used length off the job's roll.
#[tauri::command]
fn book_roll(state: Shared, inv: SharedInv) -> CmdResult<RollsView> {
    let st = state.lock().unwrap();
    let r = st.result.as_ref().ok_or("nothing nested yet")?;
    let f = st.project.check_result(r);
    if !f.fresh {
        return Err(format!("the nest is out of date ({}). Nest again before booking it.", f.reason.unwrap_or_default()));
    }
    if !r.summary.verification.ok {
        return Err("the nest failed verification, so it cannot be cut or booked".into());
    }
    let mut inv = inv.lock().unwrap();
    let roll = job_roll(&st, &inv, &r.layout.material.name)?.ok_or("pick the roll the job is cut from first")?;
    let id = roll.id.clone();
    let usage = Usage { job: job_name(&st), date: now_utc(), length_m: r.layout.used_length / 1000.0, result: result_key(&r.fingerprint, r.run.seed) };
    let mut list = inv.rolls.clone();
    plyroll_core::rolls::book(&mut list, &id, usage).map_err(err)?;
    inv.save(list).map_err(err)?;
    Ok(rolls_view(&inv))
}

/// Takes the current nest's booking back onto its roll.
#[tauri::command]
fn unbook_roll(state: Shared, inv: SharedInv) -> CmdResult<RollsView> {
    let st = state.lock().unwrap();
    let r = st.result.as_ref().ok_or("nothing nested yet")?;
    let key = result_key(&r.fingerprint, r.run.seed);
    let mut inv = inv.lock().unwrap();
    let id = inv.rolls.iter().find(|x| x.booking(&key).is_some()).map(|x| x.id.clone()).ok_or("this nest is not booked on any roll")?;
    let mut list = inv.rolls.clone();
    plyroll_core::rolls::unbook(&mut list, &id, &key).map_err(err)?;
    inv.save(list).map_err(err)?;
    Ok(rolls_view(&inv))
}

// ---- editing the result by hand ---------------------------------------------

/// Can the current result be edited? It must be current, not being replaced
/// by a running nest, and not booked (the booking holds its length).
fn editable<'a>(st: &'a mut AppState, inv: &Inventory) -> CmdResult<&'a mut NestResult> {
    if st.run.is_some() {
        return Err("a nest is running; wait for it or stop it first".into());
    }
    let r = st.result.as_ref().ok_or("nothing nested yet")?;
    let f = st.project.check_result(r);
    if !f.fresh {
        return Err(format!("the nest is out of date ({}). Nest again before editing it.", f.reason.unwrap_or_default()));
    }
    let key = result_key(&r.fingerprint, r.run.seed);
    if let Some(roll) = inv.rolls.iter().find(|x| x.booking(&key).is_some()) {
        return Err(format!("this nest is booked on roll '{}'. Take the booking back before changing the layout.", roll.id));
    }
    Ok(st.result.as_mut().expect("checked above"))
}

/// Moves, turns or locks plies of the current result (one edit undo step).
#[tauri::command]
fn edit_result(edit: Edit, state: Shared, inv: SharedInv, window: WebviewWindow) -> CmdResult<AppView> {
    let mut st = state.lock().unwrap();
    let inv = inv.lock().unwrap();
    let r = editable(&mut st, &inv)?;
    let before = r.snapshot();
    r.edit(&edit).map_err(err)?;
    if r.snapshot() != before {
        st.edit_undo.push(before);
        if st.edit_undo.len() > UNDO_LIMIT {
            st.edit_undo.remove(0);
        }
        st.edit_redo.clear();
        st.dirty = true;
    }
    Ok(view(&st, &window))
}

#[tauri::command]
fn undo_edit(redo: bool, state: Shared, inv: SharedInv, window: WebviewWindow) -> CmdResult<AppView> {
    let mut st = state.lock().unwrap();
    let inv = inv.lock().unwrap();
    let snap = if redo { st.edit_redo.pop() } else { st.edit_undo.pop() };
    let Some(snap) = snap else { return Ok(view(&st, &window)) };
    let r = match editable(&mut st, &inv) {
        Ok(r) => r,
        Err(e) => {
            // Put it back: nothing changed.
            if redo { st.edit_redo.push(snap) } else { st.edit_undo.push(snap) }
            return Err(e);
        }
    };
    let cur = r.snapshot();
    r.restore(snap);
    if redo { st.edit_undo.push(cur) } else { st.edit_redo.push(cur) }
    st.dirty = true;
    Ok(view(&st, &window))
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
    /// Kit sections of the live layout (kit by kit only).
    sections: Option<Vec<LayoutSection>>,
    /// Kit being nested (kit by kit only).
    section: Option<SectionView>,
}

#[derive(Clone, Serialize)]
struct SectionView {
    index: usize,
    count: usize,
    kit: Option<String>,
}

#[derive(Clone, Serialize)]
struct NestDone {
    run: u64,
    error: Option<String>,
    stopped: bool,
}

#[tauri::command]
/// Nests the job. With `keep_locked`, the locked plies of the current result
/// stay where they are and the rest is nested on the roll after them.
fn start_nest(keep_locked: bool, app: AppHandle, state: Shared) -> CmdResult<NestStarted> {
    let mut st = state.lock().unwrap();
    if st.run.is_some() {
        return Err("a nest is already running".into());
    }
    let input = st.project.nest_input(None).map_err(err)?;
    let keep = if keep_locked {
        let r = st.result.as_ref().ok_or("nothing nested yet")?;
        if !st.project.check_result(r).fresh {
            return Err("the nest is out of date, so its locked plies cannot be kept. Nest everything again.".into());
        }
        if r.locked.is_empty() {
            return Err("no ply is locked".into());
        }
        if !r.layout.sections.is_empty() {
            return Err("re-nesting after locked plies works on mixed layouts only, and this nest is kit by kit; nest everything again instead".into());
        }
        Some((r.layout.clone(), r.locked.clone(), r.run.edits))
    } else {
        None
    };
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
        let mut section = usize::MAX;
        let mut last_emit: Option<Instant> = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut report = |p: &Progress| {
                    // Kit by kit, the length restarts with every kit.
                    if let Some(s) = &p.section
                        && s.index != section
                    {
                        section = s.index;
                        best = f64::INFINITY;
                    }
                    let improved = p.length < best - 1e-6;
                    best = best.min(p.length);
                    let due = last_emit.is_none_or(|t| t.elapsed() >= Duration::from_millis(150));
                    if p.layout.is_some() || (improved && due) {
                        last_emit = Some(Instant::now());
                        let placements = p.layout.as_ref().map(|l| placement_views(&input.plies, l));
                        let sections = p.layout.as_ref().filter(|l| !l.sections.is_empty()).map(|l| l.sections.clone());
                        let section = p.section.as_ref().map(|s| SectionView { index: s.index, count: s.count, kit: s.kit.clone() });
                        let _ = app.emit(
                            "nest-progress",
                            NestProgress {
                                run: id,
                                phase: p.phase,
                                length: best,
                                density: p.density,
                                elapsed: p.elapsed.as_secs_f64(),
                                placements,
                                sections,
                                section,
                            },
                        );
                    }
                };
            match &keep {
                Some((layout, locked, _)) => nest_after_locked(&input.plies, layout, locked, &opts, &mut report, cancel.clone()),
                None => nest_job(&input.plies, &input.material, input.grouping, &opts, &mut report, cancel.clone()),
            }
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
                let (edits, kept) = keep.as_ref().map_or((0, 0), |(_, l, e)| (*e, l.len()));
                let run = RunInfo { seed, solve_seconds: start.elapsed().as_secs_f64(), stopped, edits, kept: kept as u32 };
                let mut result = NestResult::new(input, layout, run);
                // The kept plies come first and stay locked.
                result.locked = (0..kept).collect();
                st.result = Some(result);
                st.edit_undo.clear();
                st.edit_redo.clear();
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
fn export_result(dir: String, state: Shared, inv: SharedInv) -> CmdResult<Vec<String>> {
    let st = state.lock().unwrap();
    let r = st.result.as_ref().ok_or("nothing nested yet")?;
    let f = st.project.check_result(r);
    if !f.fresh {
        return Err(format!("the nest is out of date ({}). Nest again before exporting.", f.reason.unwrap_or_default()));
    }
    if !r.summary.verification.ok {
        return Err(format!("the nest failed verification, so no cut files are written:\n{}", r.summary.verification.issues.join("\n")));
    }
    let inv = inv.lock().unwrap();
    let roll = job_roll(&st, &inv, &r.layout.material.name)?.map(|x| RollInfo::of(x, &result_key(&r.fingerprint, r.run.seed), &today_utc()));
    let meta = ReportMeta::new(job_name(&st)).with_roll(roll);
    let opts = OutputOptions { templates: st.project.job.templates };
    let files = write_outputs(&PathBuf::from(dir), &r.plies, &r.layout, &r.summary, &r.run, &r.warnings, &meta, &opts).map_err(err)?;
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
            let rolls_path = lib_path.with_file_name("rolls.toml");
            app.manage(Mutex::new(Library::load(lib_path)));
            app.manage(Mutex::new(Inventory::load(rolls_path)));
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
            set_split,
            auto_split,
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
            get_rolls,
            save_roll,
            delete_roll,
            book_roll,
            unbook_roll,
            edit_result,
            undo_edit,
            start_nest,
            stop_nest,
            export_result,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PlyRoll");
}
