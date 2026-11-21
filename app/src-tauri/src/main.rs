//! PlyRoll desktop app. The project lives here in Rust; the web UI calls the
//! commands below and redraws from the returned view after every change.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use plyroll_core::model::Variant;
use plyroll_core::project::{ImportReport, Project, RefPick};
use plyroll_core::view::{PlyView, project_view};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State, WebviewWindow};

const UNDO_LIMIT: usize = 200;

struct AppState {
    project: Project,
    path: Option<PathBuf>,
    dirty: bool,
    undo: Vec<Project>,
    redo: Vec<Project>,
}

impl AppState {
    fn fresh() -> Self {
        AppState { project: Project::new("Untitled"), path: None, dirty: false, undo: Vec::new(), redo: Vec::new() }
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
}

fn same(a: &Project, b: &Project) -> bool {
    matches!((serde_json::to_value(a), serde_json::to_value(b)), (Ok(x), Ok(y)) if x == y)
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
}

#[derive(Serialize)]
struct ImportResult {
    view: AppView,
    report: ImportReport,
}

type Shared<'a> = State<'a, Mutex<AppState>>;
type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn view(st: &AppState, window: &WebviewWindow) -> AppView {
    let ready = match st.project.to_plies() {
        Ok((plies, warnings)) => {
            let n: u32 = plies.iter().flat_map(|p| p.variants.iter().map(|v| v.qty)).sum();
            let mut message = format!("{n} plies from {} shapes ready to nest", plies.len());
            if !warnings.is_empty() {
                message.push_str(&format!(" ({} without quantities left out)", warnings.len()));
            }
            Readiness { ok: true, message }
        }
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
    *st = AppState::fresh();
    view(&st, &window)
}

#[tauri::command]
fn open_project(path: String, state: Shared, window: WebviewWindow) -> CmdResult<AppView> {
    let path = PathBuf::from(path);
    let project = Project::load(&path).map_err(err)?;
    let mut st = state.lock().unwrap();
    *st = AppState { project, path: Some(path), dirty: false, undo: Vec::new(), redo: Vec::new() };
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
    st.project.save(&target).map_err(err)?;
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Mutex::new(AppState::fresh()));
            // `plyroll-app path/to/project.plyroll` opens it on start.
            if let Some(arg) = std::env::args().nth(1).filter(|a| a.ends_with(".plyroll")) {
                let path = PathBuf::from(&arg);
                match Project::load(&path) {
                    Ok(project) => {
                        let st = app.state::<Mutex<AppState>>();
                        *st.lock().unwrap() = AppState { project, path: Some(path), dirty: false, undo: Vec::new(), redo: Vec::new() };
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
            set_reference,
            set_variants,
            set_cosmetic,
            set_mirrored,
            rename_ply,
            remove_plies,
            undo,
            redo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PlyRoll");
}
