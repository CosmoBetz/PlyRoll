// Browser-only stand-in for the Rust backend, fed with a real project view
// dumped from Rust (src/mock/demo-view.json). For UI development only:
// geometry rules are simplified and nothing is saved.

import type { Backend } from "./backend";
import type { AppView, JobSettings, LayoutShapes, Material, NestDone, NestProgress, Placement, Ply, RefOrigin, RefPick, ResultView } from "./types";
import { longestEdge, normAxis, totalQty } from "./geo";
import demo from "../mock/demo-view.json";

// The demo material library (examples/demo/materials.toml).
const DEMO_MATERIALS: Material[] = [
  { name: "UD-T700-150", kind: "ud", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: 45 },
  { name: "Twill-200", kind: "twill", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: 38 },
  { name: "Twill-200-swap", kind: "twill", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: true, cost_per_m2: null },
];

type Snapshot = { plies: Ply[]; job: JobSettings };

const area = (pts: [number, number][]) => Math.abs(pts.reduce((s, p, i) => s + p[0] * pts[(i + 1) % pts.length][1] - pts[(i + 1) % pts.length][0] * p[1], 0) / 2);

/** Simple column packing standing in for the solver: rotate each copy so its fibre runs along the roll, stack across the width. */
function fakeNest(plies: Ply[], job: JobSettings, shuffle: number): { shapes: LayoutShapes; placements: Placement[]; length: number } {
  const m = job.material!;
  const nested = plies.filter((p) => p.total_qty > 0);
  const shapes: LayoutShapes = { material: m, shapes: nested.map((p) => ({ name: p.name, outline: p.outline, holes: p.holes })) };
  const items: { shape: number; angle: number; kit: string | null; copy: number; rot: number; w: number; h: number; minx: number; miny: number }[] = [];
  nested.forEach((p, si) => {
    for (const v of p.variants)
      for (let c = 1; c <= v.qty * job.sets; c++) {
        const rot = -((p.ref_deg ?? 0) + v.angle_deg);
        const r = (rot * Math.PI) / 180;
        const pts = p.outline.map(([x, y]) => [x * Math.cos(r) - y * Math.sin(r), x * Math.sin(r) + y * Math.cos(r)]);
        const xs = pts.map((q) => q[0]);
        const ys = pts.map((q) => q[1]);
        const minx = Math.min(...xs);
        const miny = Math.min(...ys);
        items.push({ shape: si, angle: v.angle_deg, kit: v.kit, copy: c, rot, w: Math.max(...xs) - minx, h: Math.max(...ys) - miny, minx, miny });
      }
  });
  items.sort((a, b) => b.h * b.w - a.h * a.w + (shuffle ? Math.sin(a.w * shuffle) - Math.sin(b.w * shuffle) : 0));
  const usable = m.roll_width - 2 * m.edge_trim;
  let colX = m.lead_trim;
  let colW = 0;
  let y = m.edge_trim;
  let length = m.lead_trim;
  const placements: Placement[] = [];
  for (const it of items) {
    if (y + it.h > m.edge_trim + usable + 1e-9 && y > m.edge_trim) {
      colX += colW + m.spacing;
      colW = 0;
      y = m.edge_trim;
    }
    placements.push({ shape: it.shape, angle: it.angle, kit: it.kit, copy: it.copy, rotation: it.rot, tx: colX - it.minx, ty: y - it.miny, fibre: 0, label: `${nested[it.shape].name} ${it.angle}°` });
    y += it.h + m.spacing;
    colW = Math.max(colW, it.w);
    length = Math.max(length, colX + it.w);
  }
  return { shapes, placements, length };
}

export function createMockBackend(): Backend {
  let plies: Ply[] = structuredClone(demo as unknown as Ply[]);
  let job: JobSettings = { material: null, time_s: 30, sets: 1 };
  let undo: Snapshot[] = [];
  let redo: Snapshot[] = [];
  let dirty = false;
  let library: Material[] = structuredClone(DEMO_MATERIALS);
  let result: (Omit<ResultView, "fresh" | "stale_reason"> & { key: string }) | null = null;
  let running: { id: number; stop: () => void } | null = null;
  let runSeq = 0;
  const listeners: { progress: (p: NestProgress) => void; done: (d: NestDone) => void }[] = [];

  const inputKey = () => JSON.stringify([plies.map((p) => [p.id, p.ref_deg, p.status, p.variants, p.mirrored]), job]);
  const blockedMsg = () => {
    const blocked = plies.filter((p) => p.total_qty > 0 && p.status !== "ok");
    return blocked.length ? "set the 0° reference before nesting:\n  " + blocked.map((p) => p.name).join("\n  ") : null;
  };
  const view = (): AppView => {
    const b = blockedMsg();
    const n = plies.reduce((s, p) => s + p.total_qty, 0);
    const ready = b ? { ok: false, message: b } : { ok: true, message: `${n} plies ready to nest` };
    const nest_ready = b ? ready : job.material ? { ok: true, message: `${n * job.sets} plies on ${job.material.name}` } : { ok: false, message: "choose a material for this project first" };
    const fresh = result != null && result.key === inputKey();
    return {
      name: "demo (browser mock)",
      path: null,
      dirty,
      can_undo: undo.length > 0,
      can_redo: redo.length > 0,
      plies: structuredClone(plies),
      ready,
      job: structuredClone(job),
      nest_ready,
      result: result ? { ...structuredClone(result), fresh, stale_reason: fresh ? null : "plies, quantities, sets or material changed since this nest" } : null,
      running: running?.id ?? null,
    };
  };
  const snapshot = (): Snapshot => structuredClone({ plies, job });
  const change = (ids: number[], f: (p: Ply) => void): Promise<AppView> => {
    const before = snapshot();
    try {
      for (const p of plies.filter((p) => ids.includes(p.id))) f(p);
    } catch (e) {
      plies = before.plies;
      return Promise.reject(String(e instanceof Error ? e.message : e));
    }
    undo.push(before);
    redo = [];
    dirty = true;
    return Promise.resolve(view());
  };
  const setRef = (p: Ply, deg: number, origin: RefOrigin) => {
    p.ref_deg = normAxis(deg);
    p.ref_origin = origin;
    p.status = "ok";
    p.note = null;
  };
  const dir = (a: [number, number], b: [number, number]) => (Math.atan2(b[1] - a[1], b[0] - a[0]) * 180) / Math.PI;

  return {
    kind: "mock",
    state: async () => view(),
    newProject: async () => {
      running?.stop();
      result = null;
      job = { material: null, time_s: 30, sets: 1 };
      plies = [];
      undo = [];
      redo = [];
      dirty = false;
      return view();
    },
    openProject: () => Promise.reject("opening files needs the desktop app"),
    saveProject: () => Promise.reject("saving needs the desktop app"),
    importDxf: () => Promise.reject("importing DXF needs the desktop app"),
    reload: () => Promise.reject("reloading DXF needs the desktop app"),
    setReference: (ids, pick: RefPick) => {
      if ((pick.kind === "edge" || pick.kind === "points") && ids.length !== 1) return Promise.reject("pick an edge or two points on one ply at a time");
      return change(ids, (p) => {
        switch (pick.kind) {
          case "angle":
            return setRef(p, pick.deg, "manual");
          case "drawing-x":
            return setRef(p, 0, "drawing-x");
          case "drawing-y":
            return setRef(p, 90, "drawing-y");
          case "longest-edge":
          case "bounding-box": {
            const e = longestEdge(p);
            if (!e) throw new Error(`${p.name}: outline has no straight edge`);
            return setRef(p, e.angle, pick.kind);
          }
          case "layer":
            if (p.layer_ref_deg == null) throw new Error(`${p.name}: no line on a fibre layer`);
            return setRef(p, p.layer_ref_deg, "from-layer");
          case "edge": {
            const e = p.edges.find((e) => e.contour === pick.contour && e.seg === pick.seg);
            if (!e) throw new Error("no such edge");
            return setRef(p, dir(e.a, e.b), "manual");
          }
          case "points":
            return setRef(p, dir([pick.a.x, pick.a.y], [pick.b.x, pick.b.y]), "manual");
          case "rotate":
            if (p.ref_deg == null) throw new Error(`${p.name}: set a 0° reference before turning it`);
            return setRef(p, p.ref_deg + pick.delta, "manual");
          case "confirm":
            if (p.ref_deg == null) throw new Error(`${p.name}: no 0° reference to confirm`);
            p.status = "ok";
            p.note = null;
        }
      });
    },
    setVariants: (ids, variants) =>
      change(ids, (p) => {
        p.variants = structuredClone(variants);
        p.total_qty = totalQty(variants);
      }),
    setCosmetic: (ids, on) => change(ids, (p) => void (p.cosmetic = on)),
    setMirrored: (ids, on) =>
      change(ids, (p) => {
        if (p.mirrored === on) return;
        const fx = (q: [number, number]): [number, number] => [-q[0], q[1]];
        p.mirrored = on;
        p.outline = p.outline.map(fx);
        p.holes = p.holes.map((h) => h.map(fx));
        p.marks = p.marks.map((m) => ({ ...m, pts: m.pts.map(fx) }));
        p.edges = p.edges.map((e) => ({ ...e, a: fx(e.a), b: fx(e.b), angle: normAxis(-e.angle) }));
        p.bbox = [-p.bbox[2], p.bbox[1], -p.bbox[0], p.bbox[3]];
        if (p.layer_ref_deg != null) p.layer_ref_deg = normAxis(-p.layer_ref_deg);
        if (p.ref_deg != null) {
          p.ref_deg = normAxis(-p.ref_deg);
          p.status = "check";
          p.note = on ? "mirrored; check the 0° reference" : "mirror removed; check the 0° reference";
        }
      }),
    rename: (id, name) => {
      if (!name.trim()) return Promise.reject("name must not be empty");
      return change([id], (p) => void (p.name = name.trim()));
    },
    remove: async (ids) => {
      undo.push(snapshot());
      redo = [];
      plies = plies.filter((p) => !ids.includes(p.id));
      dirty = true;
      return view();
    },
    undo: async () => {
      const prev = undo.pop();
      if (prev) {
        redo.push(snapshot());
        ({ plies, job } = prev);
        dirty = true;
      }
      return view();
    },
    redo: async () => {
      const next = redo.pop();
      if (next) {
        undo.push(snapshot());
        ({ plies, job } = next);
        dirty = true;
      }
      return view();
    },
    setJob: async (j) => {
      if (j.sets < 1 || j.time_s < 1) throw "sets and time must be at least 1";
      undo.push(snapshot());
      redo = [];
      job = structuredClone(j);
      dirty = true;
      return view();
    },
    library: async () => ({ path: "(browser mock)", materials: structuredClone(library), error: null }),
    saveMaterial: async (original, m) => {
      if (!m.name.trim()) throw "a material needs a name";
      if (m.roll_width - 2 * m.edge_trim <= 0) throw `material '${m.name}': edge_trim leaves no usable width`;
      const i = original == null ? -1 : library.findIndex((x) => x.name === original);
      if (library.some((x, k) => k !== i && x.name.toLowerCase() === m.name.trim().toLowerCase())) throw `a material named '${m.name}' already exists`;
      if (i >= 0) library[i] = structuredClone(m);
      else library.push(structuredClone(m));
      return { path: "(browser mock)", materials: structuredClone(library), error: null };
    },
    deleteMaterial: async (name) => {
      library = library.filter((m) => m.name !== name);
      return { path: "(browser mock)", materials: structuredClone(library), error: null };
    },
    importMaterials: () => Promise.reject("importing a file needs the desktop app"),
    startNest: async () => {
      if (running) throw "a nest is already running";
      const b = blockedMsg();
      if (b) throw b;
      if (!job.material) throw "choose a material for this project first";
      const id = ++runSeq;
      const key = inputKey();
      const start = Date.now();
      const total = Math.min(job.time_s, 6) * 1000;
      let step = 0;
      let best = fakeNest(plies, job, 0);
      const shapes = best.shapes;
      const finish = (stopped: boolean) => {
        clearInterval(timer);
        running = null;
        const m = job.material!;
        const parts = best.placements.reduce((s, pl) => s + area(shapes.shapes[pl.shape].outline), 0);
        const roll = best.length * m.roll_width;
        const usable = (best.length - m.lead_trim) * (m.roll_width - 2 * m.edge_trim);
        result = {
          key,
          summary: {
            used_length_mm: best.length,
            roll_area_m2: roll / 1e6,
            parts_area_m2: parts / 1e6,
            utilization_of_roll: parts / roll,
            utilization_of_usable_area: parts / usable,
            material_cost: m.cost_per_m2 == null ? null : (m.cost_per_m2 * roll) / 1e6,
            plies: best.placements.length,
            verification: { ok: true, issues: [], min_gap: m.spacing, max_fibre_deviation: 0 },
          },
          run: { seed: 1, solve_seconds: (Date.now() - start) / 1000, stopped },
          warnings: [],
          used_length: best.length,
          shapes,
          placements: best.placements,
        };
        dirty = true;
        listeners.forEach((l) => l.done({ run: id, error: null, stopped }));
      };
      const timer = setInterval(() => {
        step++;
        const next = fakeNest(plies, job, step);
        if (next.length < best.length) best = next;
        const elapsed = (Date.now() - start) / 1000;
        listeners.forEach((l) => l.progress({ run: id, phase: elapsed < (total / 1000) * 0.8 ? "explore" : "compress", length: best.length, density: 0.7, elapsed, placements: best.placements }));
        if (Date.now() - start >= total) finish(false);
      }, 400);
      running = { id, stop: () => finish(true) };
      return { run: id, time_s: job.time_s, shapes };
    },
    stopNest: async () => running?.stop(),
    onNest: async (on) => {
      listeners.push(on);
      return () => listeners.splice(listeners.indexOf(on), 1);
    },
    exportResult: () => Promise.reject("exporting cut files needs the desktop app"),
    pickProjectToOpen: async () => null,
    pickProjectToSave: async () => null,
    pickDxfFiles: async () => [],
    pickMaterialsFile: async () => null,
    pickFolder: async () => null,
    confirm: async (message) => window.confirm(message),
    onFileDrop: async () => () => {},
    onCloseRequested: async () => {},
  };
}
