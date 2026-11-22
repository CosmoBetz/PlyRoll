// Browser-only stand-in for the Rust backend, fed with a real project view
// dumped from Rust (src/mock/demo-view.json). For UI development only:
// geometry rules are simplified and nothing is saved.

import type { Backend } from "./backend";
import type { AppView, Edit, ImportSettings, JobSettings, LayoutShapes, Material, NestDone, NestProgress, Placement, Ply, RefOrigin, RefPick, ResultView, Roll, RollsView } from "./types";
import { longestEdge, normAxis, totalQty } from "./geo";
import { boxOf, clashes, placedOutline } from "./collide";
import { autoLines, cutPieces, widthAcross } from "./split";
import type { Variant, VariantFit } from "./types";

/** The mock's stand-in for the Rust fit and split (display outlines, UD rules only). */
function fitFor(p: Ply, v: Variant, m: Material | null): VariantFit | null {
  if (!m || p.ref_deg == null) return null;
  const fibre = p.ref_deg + v.angle_deg;
  const usable = m.roll_width - 2 * m.edge_trim;
  const width = widthAcross(p.outline, fibre);
  const fit: VariantFit = { fibre_deg: fibre, width, usable, fits: width <= usable + 1e-6, lines: [], pieces: [], error: null };
  const s = v.split;
  if (!s) return fit;
  fit.lines = s.mode === "auto" ? autoLines(p.outline, p.holes, fibre, usable, s.overlap, s.pieces) : s.lines;
  const pcs = fit.lines.length ? cutPieces(p.outline, p.holes, fit.lines, s.overlap) : [];
  if (pcs.length > 1)
    fit.pieces = pcs.map((pc) => {
      const w = widthAcross(pc.outline, fibre);
      return { ...pc, width: w, fits: w <= usable + 1e-6 };
    }).sort((a, b) => {
      const cy = (q: typeof a) => {
        const ys = q.outline.map(([x, y]) => -x * Math.sin((fibre * Math.PI) / 180) + y * Math.cos((fibre * Math.PI) / 180));
        return (Math.min(...ys) + Math.max(...ys)) / 2;
      };
      return cy(b) - cy(a);
    });
  return fit;
}

/** Split angles become one ply per piece, as the Rust nest input does. */
function expandForNest(plies: Ply[], m: Material): Ply[] {
  return plies.flatMap((p) => {
    const keep = { ...p, variants: p.variants.filter((v) => !v.split) };
    keep.total_qty = totalQty(keep.variants);
    const extra = p.variants.flatMap((v) => {
      if (!v.split) return [];
      const f = fitFor(p, v, m);
      if (!f || f.pieces.length < 2) return [{ ...p, variants: [{ ...v, split: null }], total_qty: v.qty }];
      return f.pieces.map((pc, k) => ({ ...p, name: `${p.name} [${k + 1}/${f.pieces.length}]`, outline: pc.outline, holes: pc.holes, variants: [{ ...v, split: null }], total_qty: v.qty }));
    });
    return [...(keep.variants.length ? [keep] : []), ...extra];
  });
}
import demo from "../mock/demo-view.json";

// The demo material library (examples/demo/materials.toml).
const DEMO_MATERIALS: Material[] = [
  { name: "UD-T700-150", kind: "ud", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: 45 },
  { name: "Twill-200", kind: "twill", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: 38 },
  { name: "Twill-200-swap", kind: "twill", roll_width: 600, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: true, cost_per_m2: null },
  // A narrow roll, so the demo plies need splitting.
  { name: "UD-narrow-250", kind: "ud", roll_width: 250, edge_trim: 10, lead_trim: 20, spacing: 2, angle_tolerance: 0, tolerance_step: 0.5, warp_weft_interchangeable: false, cost_per_m2: 45 },
];

// A small demo inventory.
const DEMO_ROLLS: Roll[] = [
  { id: "R-101", material: "UD-T700-150", batch: "B2311-07", remaining_m: 24.5, expires: "2027-05-31" },
  { id: "R-102", material: "UD-T700-150", batch: "B2311-07", remaining_m: 1.2, expires: "2027-05-31", note: "end of roll" },
  { id: "R-087", material: "UD-T700-150", batch: "B2306-02", remaining_m: 12.0, expires: "2026-10-31" },
  { id: "T-12", material: "Twill-200", batch: "TW-4471", remaining_m: 40.0 },
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
    placements.push({ shape: it.shape, angle: it.angle, kit: it.kit, copy: it.copy, rotation: it.rot, tx: colX - it.minx, ty: y - it.miny, fibre: 0, label: `${nested[it.shape].name} ${it.angle}°`, turn: 180 });
    y += it.h + m.spacing;
    colW = Math.max(colW, it.w);
    length = Math.max(length, colX + it.w);
  }
  return { shapes, placements, length };
}

export function createMockBackend(): Backend {
  let plies: Ply[] = structuredClone(demo as unknown as Ply[]);
  let job: JobSettings = { material: null, time_s: 30, sets: 1, grouping: "global" };
  let importSettings: ImportSettings = { units: "auto", join_tol: 0.01, gap_tol: 0.1, curve_tol: 0.01 };
  let undo: Snapshot[] = [];
  let redo: Snapshot[] = [];
  let dirty = false;
  let library: Material[] = structuredClone(DEMO_MATERIALS);
  let result: Omit<ResultView, "fresh" | "stale_reason"> | null = null;
  type EditSnap = { placements: Placement[]; locked: number[]; edits: number; used: number };
  let editUndo: EditSnap[] = [];
  let editRedo: EditSnap[] = [];
  let rolls: Roll[] = structuredClone(DEMO_ROLLS);
  const today = new Date().toISOString().slice(0, 10);
  const rollsView = (): RollsView => ({ path: "(browser mock)", rolls: structuredClone(rolls), error: null, today });
  let running: { id: number; stop: () => void } | null = null;
  let runSeq = 0;
  const listeners: { progress: (p: NestProgress) => void; done: (d: NestDone) => void }[] = [];

  // Roll and template paper do not change the layout, so they are left out.
  const inputKey = () => JSON.stringify([plies.map((p) => [p.id, p.ref_deg, p.status, p.variants, p.mirrored]), job.material, job.sets, job.grouping]);
  const blockedMsg = () => {
    const blocked = plies.filter((p) => p.total_qty > 0 && p.status !== "ok");
    return blocked.length ? "set the 0° reference before nesting:\n  " + blocked.map((p) => p.name).join("\n  ") : null;
  };
  const withFit = (p: Ply): Ply => ({ ...p, fit: p.variants.map((v) => fitFor(p, v, job.material)) });
  const wideMsg = () => {
    if (!job.material) return null;
    const usable = job.material.roll_width - 2 * job.material.edge_trim;
    const wide: string[] = [];
    for (const p of plies.map(withFit))
      p.variants.forEach((v, i) => {
        const f = p.fit[i];
        if (!f || v.qty === 0) return;
        if (f.pieces.length) f.pieces.forEach((pc, k) => !pc.fits && wide.push(`${p.name} [${k + 1}/${f.pieces.length}] ${v.angle_deg}° (${Math.round(pc.width)} mm)`));
        else if (!f.fits) wide.push(`${p.name} ${v.angle_deg}° (${Math.round(f.width)} mm)`);
      });
    return wide.length ? `wider than the roll (${Math.round(usable)} mm usable); split ${wide.length === 1 ? "it" : "them"} in the Plies view:\n  ${wide.join("\n  ")}` : null;
  };
  const nestMsg = (unsplit: number, m: Material) => {
    const n = expandForNest(plies, m).reduce((s, p) => s + p.total_qty, 0) * job.sets;
    return `${n} plies on ${m.name}${n !== unsplit ? ` (${unsplit} before splitting)` : ""}`;
  };
  const view = (): AppView => {
    const b = blockedMsg();
    const n = plies.reduce((s, p) => s + p.total_qty, 0);
    const ready = b ? { ok: false, message: b } : { ok: true, message: `${n} plies ready to nest` };
    const w = wideMsg();
    const nest_ready = b ? ready : !job.material ? { ok: false, message: "choose a material for this project first" } : w ? { ok: false, message: w } : { ok: true, message: nestMsg(n * job.sets, job.material) };
    const shown = plies.map(withFit);
    const too_wide = shown.reduce((s, p) => s + p.variants.filter((v, i) => v.qty > 0 && !v.split && p.fit[i] && !p.fit[i]!.fits).length, 0);
    const fresh = result != null && result.key === inputKey();
    return {
      name: "demo (browser mock)",
      path: null,
      dirty,
      can_undo: undo.length > 0,
      can_redo: redo.length > 0,
      can_undo_edit: editUndo.length > 0,
      can_redo_edit: editRedo.length > 0,
      plies: structuredClone(shown),
      too_wide,
      ready,
      job: structuredClone(job),
      import: structuredClone(importSettings),
      nest_ready,
      result: result ? { ...structuredClone(result), fresh, stale_reason: fresh ? null : "plies, quantities, sets or material changed since this nest" } : null,
      running: running?.id ?? null,
    };
  };
  const snapshot = (): Snapshot => structuredClone({ plies, job });
  type Result = Omit<ResultView, "fresh" | "stale_reason">;
  const snap = (r: Result): EditSnap => structuredClone({ placements: r.placements, locked: r.locked, edits: r.run.edits, used: r.used_length });
  const editable = (): Result => {
    if (running) throw "a nest is running; wait for it or stop it first";
    if (!result) throw "nothing nested yet";
    if (result.key !== inputKey()) throw "the nest is out of date. Nest again before editing it.";
    const key = result.key;
    const booked = rolls.find((x) => x.usage?.some((u) => u.result === key));
    if (booked) throw `this nest is booked on roll '${booked.id}'. Take the booking back before changing the layout.`;
    return result;
  };
  /** Used length, numbers and the (display-outline) check after an edit. */
  const refit = (r: Result) => {
    const m = r.shapes.material;
    r.used_length = Math.max(m.lead_trim, ...r.placements.map((pl) => boxOf(placedOutline(r.shapes, pl))[2]));
    const c = clashes(r.shapes, r.placements, null);
    const roll = r.used_length * m.roll_width;
    const usable = (r.used_length - m.lead_trim) * (m.roll_width - 2 * m.edge_trim);
    const parts = r.summary.parts_area_m2 * 1e6;
    r.summary = {
      ...r.summary,
      used_length_mm: r.used_length,
      roll_area_m2: roll / 1e6,
      utilization_of_roll: parts / roll,
      utilization_of_usable_area: parts / usable,
      material_cost: m.cost_per_m2 == null ? null : (m.cost_per_m2 * roll) / 1e6,
      verification: { ...r.summary.verification, ok: c.issues.length === 0, issues: c.issues, bad: [...c.bad].sort((a, b) => a - b) },
    };
  };
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
      job = { material: null, time_s: 30, sets: 1, grouping: "global" };
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
    setImport: async (settings) => {
      // The mock has no DXFs to re-read: only plies-free projects take new settings.
      if (plies.length) throw "re-reading DXFs with new import settings needs the desktop app";
      importSettings = structuredClone(settings);
      dirty = true;
      return { view: view(), report: { added: 0, unchanged: 0, changed: 0, rescaled: 0, warnings: [] } };
    },
    importSources: async () => {
      const files = new Map<string, number>();
      for (const p of plies) files.set(p.file, (files.get(p.file) ?? 0) + 1);
      // The demo DXFs declare mm (tab_inch declares inch).
      return [...files].map(([path, n]) => ({ path, found: true, file_units: path.includes("inch") ? "inch" : "mm", plies: n }));
    },
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
    setSplit: (id, i, split) =>
      change([id], (p) => {
        if (!p.variants[i]) throw new Error(`${p.name} has no angle ${i}`);
        p.variants[i].split = split;
      }),
    autoSplit: () => {
      if (!job.material) return Promise.reject("choose a material for this project first");
      const ids = plies.filter((p) => p.status === "ok" && p.variants.some((v) => !v.split && v.qty > 0 && !fitFor(p, v, job.material)!.fits)).map((p) => p.id);
      if (!ids.length) return Promise.reject("no angle is wider than the roll");
      return change(ids, (p) => {
        for (const v of p.variants) if (!v.split && v.qty > 0 && !fitFor(p, v, job.material)!.fits) v.split = { mode: "auto", overlap: 0, pieces: null };
      });
    },
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
    rolls: async () => rollsView(),
    saveRoll: async (original, r) => {
      if (!r.id.trim()) throw "a roll needs an id";
      if (!r.material.trim()) throw `roll '${r.id}': it needs a material`;
      if (!(r.remaining_m >= 0)) throw `roll '${r.id}': the remaining length must be between 0 and 100000 m`;
      if (r.expires && !/^\d{4}-\d{2}-\d{2}$/.test(r.expires)) throw `roll '${r.id}': the expiry date must be written YYYY-MM-DD`;
      const i = original == null ? -1 : rolls.findIndex((x) => x.id === original);
      if (rolls.some((x, k) => k !== i && x.id.toLowerCase() === r.id.trim().toLowerCase())) throw `a roll with the id '${r.id}' already exists`;
      const next: Roll = { ...structuredClone(r), id: r.id.trim(), usage: i >= 0 ? rolls[i].usage : [] };
      if (i >= 0) rolls[i] = next;
      else rolls.push(next);
      return rollsView();
    },
    deleteRoll: async (id) => {
      rolls = rolls.filter((r) => r.id !== id);
      return rollsView();
    },
    bookRoll: async () => {
      if (!result) throw "nothing nested yet";
      if (result.key !== inputKey()) throw "the nest is out of date. Nest again before booking it.";
      const r = rolls.find((x) => x.id === job.roll);
      if (!r) throw "pick the roll the job is cut from first";
      const key = result.key;
      const other = rolls.find((x) => x.usage?.some((u) => u.result === key));
      if (other) throw `this nest is already booked on roll '${other.id}'`;
      const m = result.used_length / 1000;
      if (m > r.remaining_m) throw `roll '${r.id}' has ${r.remaining_m.toFixed(2)} m left, the nest needs ${m.toFixed(2)} m. Correct the roll's remaining length first if it is wrong.`;
      r.remaining_m -= m;
      r.usage = [...(r.usage ?? []), { job: "Untitled", date: new Date().toISOString().slice(0, 16).replace("T", " ") + " UTC", length_m: m, result: key }];
      return rollsView();
    },
    unbookRoll: async () => {
      const key = result?.key;
      const r = rolls.find((x) => x.usage?.some((u) => u.result === key));
      if (!r) throw "this nest is not booked on any roll";
      const u = r.usage!.find((x) => x.result === key)!;
      r.usage = r.usage!.filter((x) => x !== u);
      r.remaining_m += u.length_m;
      return rollsView();
    },
    startNest: async (keepLocked = false) => {
      if (running) throw "a nest is already running";
      const b = blockedMsg();
      if (b) throw b;
      if (!job.material) throw "choose a material for this project first";
      const id = ++runSeq;
      const key = inputKey();
      // Keeping locked plies: they stay, everything else goes after the last of them.
      let keep: { placements: Placement[]; end: number; edits: number } | null = null;
      if (keepLocked) {
        if (!result || result.key !== key) throw "the nest is out of date, so its locked plies cannot be kept. Nest everything again.";
        if (!result.locked.length) throw "no ply is locked";
        if (result.sections.length) throw "re-nesting after locked plies works on mixed layouts only, and this nest is kit by kit; nest everything again instead";
        const r = result;
        const kept = r.locked.map((i) => structuredClone(r.placements[i]));
        if (kept.length === r.placements.length) throw "every ply is locked, so there is nothing left to nest";
        keep = { placements: kept, end: Math.max(...kept.map((pl) => boxOf(placedOutline(r.shapes, pl))[2])), edits: r.run.edits };
      }
      const around = (n: ReturnType<typeof fakeNest>) => {
        if (!keep) return n;
        const k = keep;
        const dx = k.end + job.material!.spacing - job.material!.lead_trim;
        const rest = n.placements
          .filter((pl) => !k.placements.some((q) => q.shape === pl.shape && q.angle === pl.angle && q.copy === pl.copy))
          .map((pl) => ({ ...pl, tx: pl.tx + dx }));
        return { ...n, placements: [...k.placements, ...rest], length: Math.max(k.end, n.length + dx) };
      };
      const start = Date.now();
      const total = Math.min(job.time_s, 6) * 1000;
      let step = 0;
      const nestPlies = expandForNest(plies, job.material);
      let best = around(fakeNest(nestPlies, job, 0));
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
            verification: { ok: true, issues: [], min_gap: m.spacing, max_fibre_deviation: 0, bad: [] },
          },
          run: { seed: 1, solve_seconds: (Date.now() - start) / 1000, stopped, edits: keep?.edits ?? 0, kept: keep?.placements.length ?? 0 },
          warnings: [],
          used_length: best.length,
          shapes,
          placements: best.placements,
          sections: [],
          locked: keep ? keep.placements.map((_, i) => i) : [],
        };
        editUndo = [];
        editRedo = [];
        dirty = true;
        listeners.forEach((l) => l.done({ run: id, error: null, stopped }));
      };
      const timer = setInterval(() => {
        step++;
        const next = around(fakeNest(nestPlies, job, step));
        if (next.length < best.length) best = next;
        const elapsed = (Date.now() - start) / 1000;
        listeners.forEach((l) => l.progress({ run: id, phase: elapsed < (total / 1000) * 0.8 ? "explore" : "compress", length: best.length, density: 0.7, elapsed, placements: best.placements, sections: null, section: null }));
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
    editResult: async (e: Edit) => {
      const r = editable();
      const pl = r.placements;
      const idx = e.op === "rotate" ? [e.index] : e.indices;
      if (!idx.length) throw "no ply selected";
      if (idx.some((i) => i < 0 || i >= pl.length)) throw "there is no such ply in this nest";
      const before = snap(r);
      if (e.op === "lock") {
        const set = new Set(r.locked);
        for (const i of idx) e.on ? set.add(i) : set.delete(i);
        r.locked = [...set].sort((a, b) => a - b);
      } else {
        const l = idx.find((i) => r.locked.includes(i));
        if (l != null) throw `${pl[l].label} copy ${pl[l].copy} is locked; unlock it first`;
        if (e.op === "move") {
          if (!Number.isFinite(e.dx) || !Number.isFinite(e.dy)) throw "the move is not a valid distance";
          for (const i of idx) {
            pl[i].tx += e.dx;
            pl[i].ty += e.dy;
          }
        } else {
          const p = pl[e.index];
          if (Math.abs(e.by / p.turn - Math.round(e.by / p.turn)) > 1e-9) throw `${p.label} cannot turn by ${e.by}°: its fibres would be off the roll direction`;
          // Turn about the centre of its bounding box.
          const [x0, y0, x1, y1] = boxOf(placedOutline(r.shapes, p));
          const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
          const t = (e.by * Math.PI) / 180, c = Math.cos(t), s = Math.sin(t);
          const ox = p.tx - cx, oy = p.ty - cy;
          p.tx = cx + ox * c - oy * s;
          p.ty = cy + ox * s + oy * c;
          p.rotation = (((p.rotation + e.by) % 360) + 360) % 360;
          p.fibre = normAxis(p.fibre + e.by);
        }
        r.run.edits++;
        refit(r);
      }
      editUndo.push(before);
      editRedo = [];
      dirty = true;
      return view();
    },
    undoEdit: async (redoIt: boolean) => {
      const from = redoIt ? editRedo : editUndo;
      if (!from.length) return view();
      const r = editable();
      const s = from.pop()!;
      (redoIt ? editUndo : editRedo).push(snap(r));
      r.placements = s.placements;
      r.locked = s.locked;
      r.run.edits = s.edits;
      refit(r);
      dirty = true;
      return view();
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
