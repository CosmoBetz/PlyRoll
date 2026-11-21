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

export interface AppView {
  name: string;
  path: string | null;
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
  plies: Ply[];
  ready: { ok: boolean; message: string };
}

export interface ImportReport {
  added: number;
  unchanged: number;
  changed: number;
  warnings: string[];
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
