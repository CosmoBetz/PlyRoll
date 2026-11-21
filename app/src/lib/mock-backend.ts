// Browser-only stand-in for the Rust backend, fed with a real project view
// dumped from Rust (src/mock/demo-view.json). For UI development only:
// geometry rules are simplified and nothing is saved.

import type { Backend } from "./backend";
import type { AppView, Ply, RefOrigin, RefPick } from "./types";
import { longestEdge, normAxis, totalQty } from "./geo";
import demo from "../mock/demo-view.json";

export function createMockBackend(): Backend {
  let plies: Ply[] = structuredClone(demo as unknown as Ply[]);
  let undo: Ply[][] = [];
  let redo: Ply[][] = [];
  let dirty = false;

  const view = (): AppView => {
    const blocked = plies.filter((p) => p.total_qty > 0 && p.status !== "ok");
    const ready = blocked.length
      ? { ok: false, message: "set the 0° reference before nesting:\n  " + blocked.map((p) => p.name).join("\n  ") }
      : { ok: true, message: `${plies.reduce((s, p) => s + p.total_qty, 0)} plies ready to nest` };
    return { name: "demo (browser mock)", path: null, dirty, can_undo: undo.length > 0, can_redo: redo.length > 0, plies: structuredClone(plies), ready };
  };
  const change = (ids: number[], f: (p: Ply) => void): Promise<AppView> => {
    const before = structuredClone(plies);
    try {
      for (const p of plies.filter((p) => ids.includes(p.id))) f(p);
    } catch (e) {
      plies = before;
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
      undo.push(structuredClone(plies));
      redo = [];
      plies = plies.filter((p) => !ids.includes(p.id));
      dirty = true;
      return view();
    },
    undo: async () => {
      const prev = undo.pop();
      if (prev) {
        redo.push(plies);
        plies = prev;
        dirty = true;
      }
      return view();
    },
    redo: async () => {
      const next = redo.pop();
      if (next) {
        undo.push(plies);
        plies = next;
        dirty = true;
      }
      return view();
    },
    pickProjectToOpen: async () => null,
    pickProjectToSave: async () => null,
    pickDxfFiles: async () => [],
    confirm: async (message) => window.confirm(message),
    onFileDrop: async () => () => {},
    onCloseRequested: async () => {},
  };
}
