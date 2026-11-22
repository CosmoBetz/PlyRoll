// Mirrors the Rust view model (plyroll-core view.rs, app main.rs).

export type Status = "unset" | "check" | "ok";
export type RefOrigin = "manual" | "drawing-x" | "drawing-y" | "longest-edge" | "bounding-box" | "from-layer";
export type XY = [number, number];

export interface Variant {
  angle_deg: number;
  qty: number;
  kit: string | null;
}

export interface Edge {
  /** 0 = outline, k = hole k-1 */
  contour: number;
  seg: number;
  a: XY;
  b: XY;
  len: number;
  /** degrees from drawing X, (-90, 90] */
  angle: number;
}

export interface Mark {
  pts: XY[];
  closed: boolean;
}

export interface Ply {
  id: number;
  name: string;
  file: string;
  part: string;
  status: Status;
  note: string | null;
  ref_deg: number | null;
  ref_origin: RefOrigin | null;
  layer_ref_deg: number | null;
  cosmetic: boolean;
  mirrored: boolean;
  variants: Variant[];
  total_qty: number;
  area: number;
  bbox: [number, number, number, number];
  outline: XY[];
  holes: XY[][];
  marks: Mark[];
  edges: Edge[];
}

export interface Readiness {
  ok: boolean;
  message: string;
}

export interface AppView {
  name: string;
  path: string | null;
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
  /** Hand edits of the result, apart from the project's undo. */
  can_undo_edit: boolean;
  can_redo_edit: boolean;
  plies: Ply[];
  ready: Readiness;
  job: JobSettings;
  import: ImportSettings;
  /** Plies ready and a material chosen. */
  nest_ready: Readiness;
  result: ResultView | null;
  /** Id of the running nest. */
  running: number | null;
}

export type MaterialKind = "ud" | "twill";

/** plyroll-core model.rs Material; lengths in mm, angles in degrees. */
export interface Material {
  name: string;
  kind: MaterialKind;
  roll_width: number;
  edge_trim: number;
  lead_trim: number;
  spacing: number;
  angle_tolerance: number;
  tolerance_step: number;
  warp_weft_interchangeable: boolean;
  cost_per_m2: number | null;
}

/** "global": all plies mixed; "by-kit": one section of roll per kit, one after another. */
export type Grouping = "global" | "by-kit";

/** Paper for the 1:1 templates (templates.pdf). */
export type Paper = { kind: "a4" } | { kind: "a3" } | { kind: "letter" } | { kind: "roll"; width_mm: number };

export interface JobSettings {
  material: Material | null;
  time_s: number;
  sets: number;
  grouping: Grouping;
  /** Id of the inventory roll the job is cut from (absent = none). */
  roll?: string | null;
  /** Also export templates.pdf on this paper (absent = no templates). */
  templates?: Paper | null;
}

/** One nest booked on a roll (plyroll-core rolls.rs). */
export interface Usage {
  job: string;
  date: string;
  length_m: number;
  /** ResultView.key of the nest. */
  result: string;
}

export interface Roll {
  id: string;
  /** Material name in the library. */
  material: string;
  batch: string;
  remaining_m: number;
  /** YYYY-MM-DD */
  expires?: string | null;
  note?: string;
  usage?: Usage[];
}

export interface RollsView {
  path: string;
  rolls: Roll[];
  error: string | null;
  /** YYYY-MM-DD, for expiry. */
  today: string;
}

/** One kit's stretch of roll in a kit-by-kit nest, roll x in mm. */
export interface LayoutSection {
  kit: string | null;
  start: number;
  end: number;
}

export interface SectionProgress {
  /** 0-based */
  index: number;
  count: number;
  kit: string | null;
}

export interface LibraryView {
  path: string;
  materials: Material[];
  error: string | null;
}

export interface VerifyReport {
  ok: boolean;
  issues: string[];
  min_gap: number | null;
  max_fibre_deviation: number;
  /** Placements named in an issue. */
  bad: number[];
}

export interface Summary {
  used_length_mm: number;
  roll_area_m2: number;
  parts_area_m2: number;
  utilization_of_roll: number;
  utilization_of_usable_area: number;
  material_cost: number | null;
  plies: number;
  verification: VerifyReport;
}

export interface RunInfo {
  seed: number;
  solve_seconds: number;
  stopped: boolean;
  /** Moves and turns made by hand after nesting. */
  edits: number;
  /** Locked plies kept from the previous layout; the rest was nested after them. */
  kept: number;
}

export interface ShapeGeom {
  name: string;
  outline: XY[];
  holes: XY[][];
}

export interface LayoutShapes {
  material: Material;
  shapes: ShapeGeom[];
}

/** Roll point = rotate(p, rotation°) + (tx, ty). */
export interface Placement {
  shape: number;
  angle: number;
  kit: string | null;
  copy: number;
  rotation: number;
  tx: number;
  ty: number;
  /** Fibre direction on the roll, degrees from the roll axis. */
  fibre: number;
  label: string;
  /** Smallest turn that keeps the fibres along the roll (180, or 90 on swappable twill). */
  turn: number;
}

export interface ResultView {
  fresh: boolean;
  stale_reason: string | null;
  summary: Summary;
  run: RunInfo;
  warnings: string[];
  used_length: number;
  shapes: LayoutShapes;
  placements: Placement[];
  /** Kit sections along the roll (kit-by-kit nests only). */
  sections: LayoutSection[];
  /** Identifies this nest in roll bookings. */
  key: string;
  /** Placements locked against hand edits. */
  locked: number[];
}

/** A hand edit of the result (plyroll-core edit.rs). */
export type Edit =
  | { op: "move"; indices: number[]; dx: number; dy: number }
  | { op: "rotate"; index: number; by: number }
  | { op: "lock"; indices: number[]; on: boolean };

export interface NestStarted {
  run: number;
  time_s: number;
  shapes: LayoutShapes;
}

export interface NestProgress {
  run: number;
  phase: string;
  length: number;
  density: number;
  elapsed: number;
  placements: Placement[] | null;
  sections: LayoutSection[] | null;
  section: SectionProgress | null;
}

export interface NestDone {
  run: number;
  error: string | null;
  stopped: boolean;
}

export interface ImportReport {
  added: number;
  unchanged: number;
  changed: number;
  /** Same outline at a new scale after a units change; 0° kept. */
  rescaled: number;
  warnings: string[];
}

/** "auto" reads $INSUNITS from each file (mm when unset); the others force every file. */
export type Units = "auto" | "mm" | "cm" | "m" | "inch";

/** plyroll-core project.rs ImportSettings; tolerances in mm, after scaling. */
export interface ImportSettings {
  units: Units;
  join_tol: number;
  gap_tol: number;
  curve_tol: number;
}

/** A DXF the project's plies come from. */
export interface SourceFile {
  path: string;
  found: boolean;
  /** $INSUNITS: "mm", "inch", …, "none" when unset; null when unreadable. */
  file_units: string | null;
  plies: number;
}

export type RefPick =
  | { kind: "angle"; deg: number }
  | { kind: "drawing-x" }
  | { kind: "drawing-y" }
  | { kind: "longest-edge" }
  | { kind: "bounding-box" }
  | { kind: "layer" }
  | { kind: "edge"; contour: number; seg: number }
  | { kind: "points"; a: { x: number; y: number }; b: { x: number; y: number } }
  | { kind: "rotate"; delta: number }
  | { kind: "confirm" };

/** UI state of a running nest. */
export interface LiveNest {
  run: number;
  time_s: number;
  shapes: LayoutShapes;
  placements: Placement[] | null;
  length: number | null;
  density: number | null;
  elapsed: number;
  phase: string;
  stopping: boolean;
  sections: LayoutSection[];
  section: SectionProgress | null;
}
