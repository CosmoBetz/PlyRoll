import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save, ask } from "@tauri-apps/plugin-dialog";
import type { Backend } from "./backend";
import type { AppView, ImportReport } from "./types";

type Imported = { view: AppView; report: ImportReport };

export const tauriBackend: Backend = {
  kind: "tauri",
  state: () => invoke<AppView>("get_state"),
  newProject: () => invoke<AppView>("new_project"),
  openProject: (path) => invoke<AppView>("open_project", { path }),
  saveProject: (path) => invoke<AppView>("save_project", { path }),
  importDxf: (paths) => invoke<Imported>("import_dxf", { paths }),
  reload: () => invoke<Imported>("reload_sources"),
  setReference: (ids, pick) => invoke<AppView>("set_reference", { ids, pick }),
  setVariants: (ids, variants) => invoke<AppView>("set_variants", { ids, variants }),
  setCosmetic: (ids, on) => invoke<AppView>("set_cosmetic", { ids, on }),
  setMirrored: (ids, on) => invoke<AppView>("set_mirrored", { ids, on }),
  rename: (id, name) => invoke<AppView>("rename_ply", { id, name }),
  remove: (ids) => invoke<AppView>("remove_plies", { ids }),
  undo: () => invoke<AppView>("undo"),
  redo: () => invoke<AppView>("redo"),

  async pickProjectToOpen() {
    const r = await open({ multiple: false, directory: false, filters: [{ name: "PlyRoll project", extensions: ["plyroll"] }] });
    return typeof r === "string" ? r : null;
  },
  async pickProjectToSave(suggested) {
    return (await save({ defaultPath: suggested, filters: [{ name: "PlyRoll project", extensions: ["plyroll"] }] })) ?? null;
  },
  async pickDxfFiles() {
    const r = await open({ multiple: true, directory: false, filters: [{ name: "DXF drawing", extensions: ["dxf", "DXF"] }] });
    if (!r) return [];
    return Array.isArray(r) ? r : [r];
  },
  confirm: (message, okLabel = "OK") => ask(message, { title: "PlyRoll", kind: "warning", okLabel, cancelLabel: "Cancel" }),
  async onFileDrop(cb) {
    return getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop") cb(e.payload.paths);
    });
  },
  async onCloseRequested(isDirty) {
    await getCurrentWindow().onCloseRequested(async (e) => {
      if (isDirty() && !(await ask("The project has unsaved changes. Close anyway?", { title: "PlyRoll", kind: "warning", okLabel: "Close without saving" }))) {
        e.preventDefault();
      }
    });
  },
};
