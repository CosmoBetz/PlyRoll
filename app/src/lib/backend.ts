// The UI talks to the project only through this interface: the Tauri
// implementation calls Rust commands; the mock lets the editor run in a
// plain browser (npm run dev) with real demo data.

import type { AppView, Edit, ImportReport, ImportSettings, JobSettings, LibraryView, Material, NestDone, NestProgress, NestStarted, RefPick, Roll, RollsView, SourceFile, Split, Variant } from "./types";

export interface Backend {
  readonly kind: "tauri" | "mock";
  state(): Promise<AppView>;
  newProject(): Promise<AppView>;
  openProject(path: string): Promise<AppView>;
  saveProject(path: string | null): Promise<AppView>;
  importDxf(paths: string[]): Promise<{ view: AppView; report: ImportReport }>;
  reload(): Promise<{ view: AppView; report: ImportReport }>;
  /** New units / tolerances; re-reads the project's DXFs with them (one undo step). */
  setImport(settings: ImportSettings): Promise<{ view: AppView; report: ImportReport }>;
  /** The project's DXFs and the units each declares. */
  importSources(): Promise<SourceFile[]>;
  setReference(ids: number[], pick: RefPick): Promise<AppView>;
  setVariants(ids: number[], variants: Variant[]): Promise<AppView>;
  /** Sets (or with null removes) the split of one angle of a ply. */
  setSplit(id: number, variant: number, split: Split | null): Promise<AppView>;
  /** Splits every angle that is wider than the roll and not split yet. */
  autoSplit(): Promise<AppView>;
  setCosmetic(ids: number[], on: boolean): Promise<AppView>;
  setMirrored(ids: number[], on: boolean): Promise<AppView>;
  rename(id: number, name: string): Promise<AppView>;
  remove(ids: number[]): Promise<AppView>;
  undo(): Promise<AppView>;
  redo(): Promise<AppView>;
  setJob(job: JobSettings): Promise<AppView>;
  // Material library (app config folder; same TOML as the CLI)
  library(): Promise<LibraryView>;
  /** Adds or replaces; `original` is the name before editing. */
  saveMaterial(original: string | null, m: Material): Promise<LibraryView>;
  deleteMaterial(name: string): Promise<LibraryView>;
  importMaterials(path: string): Promise<LibraryView>;
  // Roll inventory (rolls.toml next to the material library)
  rolls(): Promise<RollsView>;
  /** Adds or replaces; `original` is the id before editing. Usage history is kept by the backend. */
  saveRoll(original: string | null, r: Roll): Promise<RollsView>;
  deleteRoll(id: string): Promise<RollsView>;
  /** Takes the current nest's used length off the job's roll. */
  bookRoll(): Promise<RollsView>;
  /** Puts the current nest's booking back on its roll. */
  unbookRoll(): Promise<RollsView>;
  // Nesting
  /** With `keepLocked`, the current result's locked plies stay and the rest is nested after them. */
  startNest(keepLocked?: boolean): Promise<NestStarted>;
  stopNest(): Promise<void>;
  onNest(on: { progress: (p: NestProgress) => void; done: (d: NestDone) => void }): Promise<() => void>;
  /** Moves, turns or locks plies of the current result (its own undo). */
  editResult(edit: Edit): Promise<AppView>;
  undoEdit(redo: boolean): Promise<AppView>;
  /** Writes the cut files of the current result; returns the paths. */
  exportResult(dir: string): Promise<string[]>;
  // Native dialogs
  pickProjectToOpen(): Promise<string | null>;
  pickProjectToSave(suggested: string): Promise<string | null>;
  pickDxfFiles(): Promise<string[]>;
  pickMaterialsFile(): Promise<string | null>;
  /** Opens at `defaultPath` when given (e.g. the project's folder). */
  pickFolder(defaultPath?: string): Promise<string | null>;
  confirm(message: string, okLabel?: string): Promise<boolean>;
  /** Files dropped on the window (paths). Returns an unsubscribe function. */
  onFileDrop(cb: (paths: string[]) => void): Promise<() => void>;
  /** Asks before closing with unsaved changes. */
  onCloseRequested(isDirty: () => boolean): Promise<void>;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function createBackend(): Promise<Backend> {
  if (isTauri()) {
    const m = await import("./tauri-backend");
    return m.tauriBackend;
  }
  const m = await import("./mock-backend");
  return m.createMockBackend();
}
