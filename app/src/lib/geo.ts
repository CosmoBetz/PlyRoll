// Drawing coordinates are mm with y up. SVG views flip y once, in a single
// <g transform="scale(1,-1)">, so every angle here is CCW from drawing X.

import type { Edge, Ply, RefPick, Variant, XY } from "./types";

/** Axis direction normalised to (-90, 90]. */
export function normAxis(deg: number): number {
  let r = ((((deg + 90) % 180) + 180) % 180) - 90;
  if (r <= -90 + 1e-9) r = 90;
  r = Math.round(r * 1e6) / 1e6;
  return r === 0 ? 0 : r;
}

export function fmtAngle(deg: number, signed = false): string {
  const r = Math.round(deg * 100) / 100;
  const s = Number.isInteger(r) ? r.toFixed(0) : r.toFixed(r * 10 === Math.round(r * 10) ? 1 : 2);
  return (signed && r > 0 ? "+" : "") + s.replace("-", "−") + "°";
}

const PALETTE = ["#e45756", "#54a24b", "#4c78a8", "#f2b701", "#b279a2", "#72b7b2", "#ff9da6", "#9d755d"];

/** Same colours as the CLI's orientation.svg. */
export function angleColor(angle: number): string {
  const a = ((Math.round(angle) % 360) + 360) % 360;
  switch (a) {
    case 0:
    case 180:
      return PALETTE[0];
    case 45:
    case 225:
      return PALETTE[1];
    case 135:
    case 315:
      return PALETTE[2];
    case 90:
    case 270:
      return PALETTE[3];
    default:
      return PALETTE[4 + (a % 4)];
  }
}

export function pathD(pts: XY[], closed = true): string {
  if (pts.length === 0) return "";
  let d = `M${pts[0][0]} ${pts[0][1]}`;
  for (let i = 1; i < pts.length; i++) d += `L${pts[i][0]} ${pts[i][1]}`;
  return closed ? d + "Z" : d;
}

/** Outline plus holes, for fill-rule evenodd. */
export function plyD(p: Ply): string {
  return pathD(p.outline) + p.holes.map((h) => pathD(h)).join("");
}

export function centre(p: Ply): XY {
  const [x0, y0, x1, y1] = p.bbox;
  return [(x0 + x1) / 2, (y0 + y1) / 2];
}

export function diag(p: Ply): number {
  const [x0, y0, x1, y1] = p.bbox;
  return Math.hypot(x1 - x0, y1 - y0);
}

/** Parallel lines at `deg` covering the bounding box, spaced `gap`. */
export function hatchD(p: Ply, deg: number, gap: number): string {
  const [cx, cy] = centre(p);
  const r = diag(p) / 2 + gap;
  const t = (deg * Math.PI) / 180;
  const ux = Math.cos(t), uy = Math.sin(t);
  const nx = -uy, ny = ux;
  const n = Math.ceil(r / gap);
  let d = "";
  for (let i = -n; i <= n; i++) {
    const ox = cx + nx * i * gap, oy = cy + ny * i * gap;
    d += `M${ox - ux * r} ${oy - uy * r}L${ox + ux * r} ${oy + uy * r}`;
  }
  return d;
}

export function totalQty(vs: Variant[]): number {
  return vs.reduce((s, v) => s + v.qty, 0);
}

export function variantLabel(v: Variant): string {
  return `${fmtAngle(v.angle_deg, true)} ×${v.qty}`;
}

export const ORIGIN_LABEL: Record<string, string> = {
  manual: "picked / typed",
  "drawing-x": "drawing X",
  "drawing-y": "drawing Y",
  "longest-edge": "longest edge",
  "bounding-box": "bounding box",
  "from-layer": "fibre layer in DXF",
};

/** Longest straight edge of the outline (mock / hints). */
export function longestEdge(p: Ply): Edge | null {
  let best: Edge | null = null;
  for (const e of p.edges) if (e.contour === 0 && (!best || e.len > best.len)) best = e;
  return best;
}

export function pickLabel(pick: RefPick): string {
  switch (pick.kind) {
    case "angle":
      return `0° set to ${fmtAngle(pick.deg)}`;
    case "edge":
      return "0° set along the edge";
    case "points":
      return "0° set through two points";
    case "rotate":
      return `0° turned ${fmtAngle(pick.delta, true)}`;
    case "confirm":
      return "0° confirmed";
    default:
      return `0° set from ${ORIGIN_LABEL[pick.kind === "layer" ? "from-layer" : pick.kind]}`;
  }
}
