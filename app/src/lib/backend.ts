// The UI talks to the project only through this interface: the Tauri
// implementation calls Rust commands; the mock lets the editor run in a
// plain browser (npm run dev) with real demo data.

import type { AppView, ImportReport, RefPick, Variant } from "./types";

export interface Backend {
  readonly kind: "tauri" | "mock";
  state(): Promise<AppView>;
  newProject(): Promise<AppView>;
  openProject(path: string): Promise<AppView>;
  saveProject(path: string | null): Promise<AppView>;
  importDxf(paths: string[]): Promise<{ view: AppView; report: ImportReport }>;
  reload(): Promise<{ view: AppView; report: ImportReport }>;
  setReference(ids: number[], pick: RefPick): Promise<AppView>;
  setVariants(ids: number[], variants: Variant[]): Promise<AppView>;
  setCosmetic(ids: number[], on: boolean): Promise<AppView>;
  setMirrored(ids: number[], on: boolean): Promise<AppView>;
  rename(id: number, name: string): Promise<AppView>;
  remove(ids: number[]): Promise<AppView>;
  undo(): Promise<AppView>;
  redo(): Promise<AppView>;
  // Native dialogs
  pickProjectToOpen(): Promise<string | null>;
  pickProjectToSave(suggested: string): Promise<string | null>;
  pickDxfFiles(): Promise<string[]>;
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
