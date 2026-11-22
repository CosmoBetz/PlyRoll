// Splitting on the flattened display outlines: live piece widths while a cut
// line is dragged, and the browser mock's stand-in for the Rust split. The
// Rust side cuts the exact geometry and decides what is nested.

import type { CutLine, XY } from "./types";

export const rad = (deg: number) => (deg * Math.PI) / 180;

/** Turns a point by `deg` about the origin. */
export function rot([x, y]: XY, deg: number): XY {
  const c = Math.cos(rad(deg)), s = Math.sin(rad(deg));
  return [x * c - y * s, x * s + y * c];
}

/** Into the roll view of a fibre direction: the fibre runs along x. */
export const toRoll = (pts: XY[], fibre: number): XY[] => pts.map((p) => rot(p, -fibre));

export function yExtent(pts: XY[]): [number, number] {
  let lo = Infinity, hi = -Infinity;
  for (const p of pts) {
    if (p[1] < lo) lo = p[1];
    if (p[1] > hi) hi = p[1];
  }
  return [lo, hi];
}

/** Width across the roll with the fibre along it. */
export function widthAcross(pts: XY[], fibre: number): number {
  const [lo, hi] = yExtent(toRoll(pts, fibre));
  return hi - lo;
}

function signedArea(pts: XY[]): number {
  let s = 0;
  for (let i = 0; i < pts.length; i++) {
    const a = pts[i], b = pts[(i + 1) % pts.length];
    s += a[0] * b[1] - b[0] * a[1];
  }
  return s / 2;
}

/** Keeps the part of `poly` where (q − p) · n ≥ 0 (Sutherland–Hodgman). */
export function clipHalf(poly: XY[], p: XY, n: XY): XY[] {
  const d = (q: XY) => (q[0] - p[0]) * n[0] + (q[1] - p[1]) * n[1];
  const out: XY[] = [];
  for (let i = 0; i < poly.length; i++) {
    const a = poly[i], b = poly[(i + 1) % poly.length];
    const da = d(a), db = d(b);
    if (da >= 0) out.push(a);
    if ((da >= 0) !== (db >= 0)) {
      const t = da / (da - db);
      out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
    }
  }
  return out;
}

export interface Piece {
  outline: XY[];
  holes: XY[][];
}

/** Cuts an outline (and holes) by infinite lines; with an overlap each piece reaches half of it past every cut. */
export function cutPieces(outline: XY[], holes: XY[][], lines: CutLine[], overlap: number): Piece[] {
  let pieces: Piece[] = [{ outline, holes }];
  for (const l of lines) {
    const d: XY = [Math.cos(rad(l.dir_deg)), Math.sin(rad(l.dir_deg))];
    const n: XY = [-d[1], d[0]];
    const next: Piece[] = [];
    for (const pc of pieces) {
      for (const s of [1, -1]) {
        // Keep the side s, reaching o/2 past the line.
        const p: XY = [l.p.x - (s * n[0] * overlap) / 2, l.p.y - (s * n[1] * overlap) / 2];
        const nn: XY = [s * n[0], s * n[1]];
        const o = clipHalf(pc.outline, p, nn);
        if (o.length < 3 || Math.abs(signedArea(o)) < 1) continue;
        const hs = pc.holes.map((h) => clipHalf(h, p, nn)).filter((h) => h.length >= 3 && Math.abs(signedArea(h)) > 1e-6);
        next.push({ outline: o, holes: hs });
      }
    }
    pieces = next;
  }
  return pieces;
}

/** Length of the line y = `y` inside the polygons. */
function chord(polys: XY[][], y: number): number {
  const xs: number[] = [];
  for (const poly of polys)
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i], b = poly[(i + 1) % poly.length];
      if (a[1] > y !== b[1] > y) xs.push(a[0] + ((y - a[1]) * (b[0] - a[0])) / (b[1] - a[1]));
    }
  xs.sort((a, b) => a - b);
  let s = 0;
  for (let i = 0; i + 1 < xs.length; i += 2) s += xs[i + 1] - xs[i];
  return s;
}

export const AUTO_MARGIN = 0.5;

/** Automatic cuts along the fibre: as few pieces as fit (or `pieces`), each where the cut is shortest. */
export function autoLines(outline: XY[], holes: XY[][], fibre: number, usableWidth: number, overlap: number, pieces?: number | null): CutLine[] {
  const polys = [toRoll(outline, fibre), ...holes.map((h) => toRoll(h, fibre))];
  const [y0, y1] = yExtent(polys[0]);
  const w = y1 - y0;
  const usable = usableWidth - AUTO_MARGIN;
  const fitN = w <= usableWidth + 1e-6 ? 1 : Math.max(2, Math.ceil((w - overlap) / (usable - overlap)));
  const n = Math.max(fitN, pieces ?? fitN);
  if (n <= 1) return [];
  const p = (w + (n - 1) * overlap) / n;
  const c = Array.from({ length: n + 1 }, (_, k) => y0 + overlap / 2 + k * (p - overlap));
  c[n] = y1 - overlap / 2;
  // More pieces than needed: keep them close to even (narrower pieces were asked for).
  const minW = Math.max(n > fitN ? p * 0.85 : p * 0.4, overlap + 1);
  const maxW = n > fitN ? Math.min(usable, p * 1.15) : Math.max(usable, p);
  for (let pass = 0; pass < 3; pass++)
    for (let k = 1; k < n; k++) {
      const lo = Math.max(c[k - 1] + minW - overlap, c[k + 1] - (maxW - overlap));
      const hi = Math.min(c[k - 1] + maxW - overlap, c[k + 1] - (minW - overlap));
      if (hi <= lo) continue;
      let best = { len: chord(polys, c[k]), off: Math.abs(c[k] - (lo + hi) / 2), y: c[k] };
      for (let s = 0; s <= 80; s++) {
        const y = lo + ((hi - lo) * s) / 80;
        const cand = { len: chord(polys, y), off: Math.abs(y - (lo + hi) / 2), y };
        if (cand.len < best.len - 0.5 || (Math.abs(cand.len - best.len) <= 0.5 && cand.off < best.off)) best = cand;
      }
      c[k] = best.y;
    }
  const xs = polys[0].map((q) => q[0]);
  const cx = (Math.min(...xs) + Math.max(...xs)) / 2;
  return c.slice(1, n).map((y) => {
    const [x, yy] = rot([cx, y], fibre);
    return { p: { x, y: yy }, dir_deg: fibre };
  });
}
