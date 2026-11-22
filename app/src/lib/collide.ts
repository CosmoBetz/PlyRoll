// Quick spacing and roll-area checks on the display outlines, for the live
// hint while dragging plies (and the browser mock's verification). The display
// outlines are simplified, so this is a hint only: the Rust verification on the
// exact geometry decides what may be exported.

import type { LayoutShapes, Material, Placement, XY } from "./types";

export type Box = [number, number, number, number];

/** A placement's outline on the roll, moved by (dx, dy). */
export function placedOutline(shapes: LayoutShapes, pl: Placement, dx = 0, dy = 0): XY[] {
  const t = (pl.rotation * Math.PI) / 180;
  const c = Math.cos(t), s = Math.sin(t);
  return shapes.shapes[pl.shape].outline.map(([x, y]) => [x * c - y * s + pl.tx + dx, x * s + y * c + pl.ty + dy]);
}

export function boxOf(pts: XY[]): Box {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of pts) {
    if (x < x0) x0 = x;
    if (y < y0) y0 = y;
    if (x > x1) x1 = x;
    if (y > y1) y1 = y;
  }
  return [x0, y0, x1, y1];
}

const near = (a: Box, b: Box, d: number) => a[0] <= b[2] + d && b[0] <= a[2] + d && a[1] <= b[3] + d && b[1] <= a[3] + d;

function insidePoly(p: XY, poly: XY[]): boolean {
  let c = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i], [xj, yj] = poly[j];
    if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) c = !c;
  }
  return c;
}

function segPoint(p: XY, a: XY, b: XY): number {
  const dx = b[0] - a[0], dy = b[1] - a[1];
  const l = dx * dx + dy * dy;
  const t = l > 0 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l)) : 0;
  return Math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy);
}

function cross(o: XY, a: XY, b: XY): number {
  return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
}

function segSeg(a: XY, b: XY, c: XY, d: XY): number {
  const d1 = cross(c, d, a), d2 = cross(c, d, b), d3 = cross(a, b, c), d4 = cross(a, b, d);
  if (((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))) return 0;
  return Math.min(segPoint(a, c, d), segPoint(b, c, d), segPoint(c, a, b), segPoint(d, a, b));
}

/** A placement's holes on the roll, moved by (dx, dy). */
export function placedHoles(shapes: LayoutShapes, pl: Placement, dx = 0, dy = 0): XY[][] {
  const t = (pl.rotation * Math.PI) / 180;
  const c = Math.cos(t), s = Math.sin(t);
  return shapes.shapes[pl.shape].holes.map((h) => h.map(([x, y]) => [x * c - y * s + pl.tx + dx, x * s + y * c + pl.ty + dy] as XY));
}

/**
 * Smallest distance between two parts, stopping early once it is below `limit`
 * (0 when they overlap). A part inside the other's hole is measured to that hole.
 */
export function gap(a: XY[], ab: Box, b: XY[], bb: Box, limit: number, aHoles: XY[][] = [], bHoles: XY[][] = []): number {
  if (insidePoly(b[0], a)) {
    const h = aHoles.find((h) => insidePoly(b[0], h));
    return h ? edgeGap(b, bb, h, boxOf(h), limit) : 0;
  }
  if (insidePoly(a[0], b)) {
    const h = bHoles.find((h) => insidePoly(a[0], h));
    return h ? edgeGap(a, ab, h, boxOf(h), limit) : 0;
  }
  return edgeGap(a, ab, b, bb, limit);
}

/** Smallest distance between the edges of two outlines. */
function edgeGap(a: XY[], ab: Box, b: XY[], bb: Box, limit: number): number {
  // Only edges near the other outline's box can be close to it.
  const edges = (p: XY[], other: Box) => {
    const out: [XY, XY][] = [];
    for (let i = 0; i < p.length; i++) {
      const s = p[i], e = p[(i + 1) % p.length];
      const box: Box = [Math.min(s[0], e[0]), Math.min(s[1], e[1]), Math.max(s[0], e[0]), Math.max(s[1], e[1])];
      if (near(box, other, limit)) out.push([s, e]);
    }
    return out;
  };
  const ea = edges(a, bb), eb = edges(b, ab);
  let best = Infinity;
  for (const [p, q] of ea)
    for (const [r, s] of eb) {
      const d = segSeg(p, q, r, s);
      if (d < best) {
        best = d;
        if (best < limit * 0.5) return best;
      }
    }
  return best;
}

export interface Clash {
  /** Placements too close to another or off the usable roll. */
  bad: Set<number>;
  issues: string[];
}

/**
 * Checks `moved` placements (all of them when null), shifted by (dx, dy),
 * against every other placement and the usable roll area.
 */
export function clashes(shapes: LayoutShapes, placements: Placement[], moved: number[] | null, dx = 0, dy = 0, slack = 0.05): Clash {
  const m: Material = shapes.material;
  const set = new Set(moved ?? placements.map((_, i) => i));
  const pts = placements.map((pl, i) => placedOutline(shapes, pl, set.has(i) ? dx : 0, set.has(i) ? dy : 0));
  const boxes = pts.map(boxOf);
  const holes = placements.map((pl, i) => placedHoles(shapes, pl, set.has(i) ? dx : 0, set.has(i) ? dy : 0));
  const bad = new Set<number>();
  const issues: string[] = [];
  const name = (i: number) => `${placements[i].label} copy ${placements[i].copy}`;
  const need = m.spacing - slack;
  for (const i of set) {
    const b = boxes[i];
    if (b[0] < m.lead_trim - slack || b[1] < m.edge_trim - slack || b[3] > m.roll_width - m.edge_trim + slack) {
      bad.add(i);
      issues.push(`${name(i)} leaves the usable roll area`);
    }
    for (let j = 0; j < placements.length; j++) {
      if (j === i || (set.has(j) && j < i) || !near(b, boxes[j], m.spacing)) continue;
      const d = gap(pts[i], b, pts[j], boxes[j], m.spacing, holes[i], holes[j]);
      if (d < need) {
        bad.add(i);
        bad.add(j);
        issues.push(`${name(i)} and ${name(j)} are ${d.toFixed(3)} mm apart (need ${m.spacing} mm)`);
      }
    }
  }
  return { bad, issues };
}
