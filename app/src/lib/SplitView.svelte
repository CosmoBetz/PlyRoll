<script lang="ts">
  // Splitting one angle of a ply into pieces that fit the roll. The ply is
  // drawn as it lies on the roll (fibre along x, y up), so cut lines along the
  // fibre are horizontal. Lines are edited in this roll view and sent back in
  // drawing coordinates.
  import type { CutLine, Ply, Split, VariantFit, XY } from "./types";
  import { angleColor, fmtAngle, pathD } from "./geo";
  import { cutPieces, rot, toRoll, yExtent } from "./split";

  let {
    ply,
    vi,
    fit,
    onsplit,
    onclose,
  }: {
    ply: Ply;
    vi: number;
    fit: VariantFit;
    onsplit: (split: Split | null) => void;
    onclose: () => void;
  } = $props();

  /** A cut line in the roll view: through `c`, at `a` degrees to the fibre. */
  type RLine = { c: XY; a: number };

  const v = $derived(ply.variants[vi]);
  const split = $derived(v?.split ?? null);
  const f = $derived(fit.fibre_deg);
  const overlap = $derived(split?.overlap ?? 0);
  const outline = $derived(toRoll(ply.outline, f));
  const holes = $derived(ply.holes.map((h) => toRoll(h, f)));

  const box = $derived.by(() => {
    const xs = outline.map((p) => p[0]);
    const [y0, y1] = yExtent(outline);
    return { x0: Math.min(...xs), x1: Math.max(...xs), y0, y1 };
  });
  const pad = $derived(Math.max(box.x1 - box.x0, box.y1 - box.y0) * 0.12 + 10);
  // Room on the right for the width labels.
  const vb = $derived({ x: box.x0 - pad, y: -box.y1 - pad, w: box.x1 - box.x0 + 2 * pad + pad * 1.6, h: box.y1 - box.y0 + 2 * pad });

  const fromFit = (ls: CutLine[]): RLine[] => ls.map((l) => ({ c: rot([l.p.x, l.p.y], -f), a: l.dir_deg - f }));
  const toDrawing = (ls: RLine[]): CutLine[] =>
    ls.map((l) => {
      const [x, y] = rot(l.c, f);
      return { p: { x, y }, dir_deg: l.a + f };
    });

  let draft = $state<RLine[] | null>(null);
  let selected = $state<number | null>(null);
  const lines = $derived(draft ?? fromFit(fit.lines));

  // A new ply or angle starts with nothing selected.
  let shownFor = "";
  $effect(() => {
    const k = `${ply.id}:${vi}`;
    if (k !== shownFor) {
      shownFor = k;
      selected = null;
      draft = null;
    }
  });

  // Pieces: from the backend when settled, estimated on the display outline while dragging.
  type Shown = { outline: XY[]; holes: XY[][]; width: number; fits: boolean };
  const pieces = $derived.by((): Shown[] => {
    if (draft) {
      const cut = draft.map((l) => ({ p: { x: l.c[0], y: l.c[1] }, dir_deg: l.a }));
      return cutPieces(outline, holes, cut, overlap).map((p) => {
        const [lo, hi] = yExtent(p.outline);
        return { ...p, width: hi - lo, fits: hi - lo <= fit.usable + 1e-6 };
      });
    }
    if (fit.pieces.length) return fit.pieces.map((p) => ({ outline: toRoll(p.outline, f), holes: p.holes.map((h) => toRoll(h, f)), width: p.width, fits: p.fits }));
    return [{ outline, holes, width: fit.width, fits: fit.fits }];
  });
  const allFit = $derived(pieces.every((p) => p.fits));
  // No red: red means a piece does not fit.
  const COLORS = ["#4c78a8", "#54a24b", "#b279a2", "#72b7b2", "#f2b701", "#9d755d"];

  // Line geometry in the roll view.
  const dirOf = (l: RLine): XY => [Math.cos((l.a * Math.PI) / 180), Math.sin((l.a * Math.PI) / 180)];
  const anchor = (l: RLine): XY => {
    const d = dirOf(l);
    const m: XY = [(box.x0 + box.x1) / 2, (box.y0 + box.y1) / 2];
    const t = (m[0] - l.c[0]) * d[0] + (m[1] - l.c[1]) * d[1];
    return [l.c[0] + t * d[0], l.c[1] + t * d[1]];
  };
  const reach = $derived((box.x1 - box.x0) / 2 + pad * 0.55);
  const ends = (l: RLine): [XY, XY] => {
    const m = anchor(l), d = dirOf(l);
    const r = reach / Math.max(0.2, Math.abs(d[0]));
    return [
      [m[0] - d[0] * r, m[1] - d[1] * r],
      [m[0] + d[0] * r, m[1] + d[1] * r],
    ];
  };

  let svg = $state<SVGSVGElement>();
  function rollPoint(e: PointerEvent): XY {
    const pt = svg!.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const q = pt.matrixTransform(svg!.getScreenCTM()!.inverse());
    return [q.x, -q.y];
  }

  let drag: { kind: "move" | "turn"; i: number; start: XY; from: RLine; end: number } | null = null;
  function startDrag(e: PointerEvent, i: number, kind: "move" | "turn", end = 1) {
    e.stopPropagation();
    selected = i;
    drag = { kind, i, start: rollPoint(e), from: { ...lines[i], c: [...lines[i].c] as XY }, end };
    draft = lines.map((l) => ({ c: [...l.c] as XY, a: l.a }));
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }
  function onmove(e: PointerEvent) {
    if (!drag || !draft) return;
    const p = rollPoint(e);
    const l = draft[drag.i];
    if (drag.kind === "move") {
      const d = dirOf(drag.from);
      const n: XY = [-d[1], d[0]];
      const off = (p[0] - drag.start[0]) * n[0] + (p[1] - drag.start[1]) * n[1];
      l.c = [drag.from.c[0] + n[0] * off, drag.from.c[1] + n[1] * off];
    } else {
      // Turn about the line's middle; near the fibre direction it snaps back to it.
      const m = anchor(drag.from);
      let a = (Math.atan2(p[1] - m[1], p[0] - m[0]) * 180) / Math.PI;
      if (drag.end < 0) a += 180;
      a = ((a + 90) % 180 + 180) % 180 - 90;
      if (Math.abs(a) < 2) a = 0;
      a = Math.max(-80, Math.min(80, a));
      l.c = m;
      l.a = a;
    }
  }
  function onup() {
    if (!drag || !draft) return;
    const moved = drag.kind === "turn" || draft[drag.i].c.some((x, k) => Math.abs(x - drag!.from.c[k]) > 1e-6);
    drag = null;
    if (moved) commitLines(draft);
    else draft = null;
  }

  function commitLines(ls: RLine[]) {
    draft = ls;
    onsplit(ls.length ? { mode: "manual", lines: toDrawing(ls), overlap } : null);
  }
  // The backend's answer replaces the draft.
  $effect(() => {
    void fit;
    if (!drag) draft = null;
  });

  function addLine() {
    // Through the middle of the widest piece, along the fibre.
    const widest = pieces.reduce((a, b) => (b.width > a.width ? b : a), pieces[0]);
    const [lo, hi] = yExtent(widest.outline);
    const next = [...lines, { c: [(box.x0 + box.x1) / 2, (lo + hi) / 2] as XY, a: 0 }];
    selected = next.length - 1;
    commitLines(next);
  }
  function removeSelected() {
    if (selected == null) return;
    const next = lines.filter((_, k) => k !== selected);
    selected = null;
    commitLines(next);
  }
  function nudge(by: number) {
    if (selected == null) return;
    const next = lines.map((l) => ({ c: [...l.c] as XY, a: l.a }));
    const d = dirOf(next[selected]);
    next[selected].c = [next[selected].c[0] - d[1] * by, next[selected].c[1] + d[0] * by];
    commitLines(next);
  }
  const count = $derived(fit.pieces.length || 1);
  function auto(pieces?: number) {
    onsplit({ mode: "auto", overlap, pieces: pieces ?? null });
  }
  function setOverlap(raw: string) {
    const o = Math.max(0, Number(raw.replace(",", ".")) || 0);
    if (!split) onsplit({ mode: "auto", overlap: o, pieces: null });
    else onsplit({ ...split, overlap: o });
  }

  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "TEXTAREA") return;
    if ((e.key === "Delete" || e.key === "Backspace") && selected != null) {
      e.preventDefault();
      removeSelected();
    } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && selected != null) {
      e.preventDefault();
      nudge((e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : e.altKey ? 0.1 : 1));
    } else if (e.key === "Escape") {
      if (selected != null) selected = null;
      else onclose();
    }
  }

  const px = $derived(vb.w / 900); // about one screen pixel at a typical size
  const fmt = (mm: number) => (mm >= 100 ? mm.toFixed(0) : mm.toFixed(1));
</script>

<svelte:window {onkeydown} />

<div class="split">
  <div class="bar">
    <span class="what">
      <span class="swatch" style="background:{angleColor(v.angle_deg)}"></span>
      <b>{fmtAngle(v.angle_deg, true)}</b>
      <span class="muted">{fmt(fit.width)} mm across · roll {fmt(fit.usable)} mm usable</span>
    </span>
    <span class="group">
      <button class:on={split?.mode === "auto"} onclick={() => auto()} title="Cuts along the fibre, as few pieces as fit, each where the cut is shortest">Auto</button>
      <span class="stepper" title="Number of pieces (automatic cuts)">
        <button onclick={() => auto(Math.max(1, count - 1))} disabled={count <= 1} aria-label="Fewer pieces">−</button>
        <span>{count} piece{count === 1 ? "" : "s"}</span>
        <button onclick={() => auto(count + 1)} aria-label="More pieces">+</button>
      </span>
      <button onclick={addLine} title="Add a cut line along the fibre">Add line</button>
      <button onclick={removeSelected} disabled={selected == null} title="Delete the selected line (⌫)">Delete line</button>
      <button onclick={() => onsplit(null)} disabled={!split}>No split</button>
    </span>
    <label class="overlap" title="The pieces overlap by this much at each cut (0 = butt joint)">
      Overlap <input value={overlap} inputmode="decimal" onchange={(e) => setOverlap(e.currentTarget.value)} /> mm
    </label>
    <span class="spacer"></span>
    <button class="primary" onclick={onclose}>Done</button>
  </div>

  <div class="canvas">
    <svg bind:this={svg} viewBox="{vb.x} {vb.y} {vb.w} {vb.h}" preserveAspectRatio="xMidYMid meet" onpointermove={onmove} onpointerup={onup} role="application" aria-label="Cut lines">
      <g transform="scale(1 -1)">
        {#each pieces as p, k}
          <path d={pathD(p.outline) + p.holes.map((h) => pathD(h)).join("")} fill-rule="evenodd" class="piece" class:bad={!p.fits} style="--c:{COLORS[k % COLORS.length]}" />
        {/each}
        <path d={pathD(outline) + holes.map((h) => pathD(h)).join("")} fill-rule="evenodd" class="outline" style="stroke-width:{px * 1.5}" />
        {#each lines as l, i}
          {@const [a, b] = ends(l)}
          <line x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} class="cut" class:sel={selected === i} style="stroke-width:{px * (selected === i ? 2.5 : 1.8)}" />
          <line
            x1={a[0]}
            y1={a[1]}
            x2={b[0]}
            y2={b[1]}
            class="hit"
            style="stroke-width:{px * 14}"
            role="slider"
            aria-label="Cut line {i + 1}"
            aria-valuenow={Math.round(l.c[1])}
            tabindex="-1"
            onpointerdown={(e) => startDrag(e, i, "move")}
          />
          {#each [[a, -1], [b, 1]] as [q, end]}
            <circle
              cx={(q as XY)[0]}
              cy={(q as XY)[1]}
              r={px * 6}
              class="handle"
              class:sel={selected === i}
              style="stroke-width:{px * 1.5}"
              role="slider"
              aria-label="Tilt cut line {i + 1}"
              aria-valuenow={Math.round(l.a)}
              tabindex="-1"
              onpointerdown={(e) => startDrag(e, i, "turn", end as number)}
            />
          {/each}
        {/each}
        <!-- Width of each piece across the roll -->
        {#each pieces as p, k}
          {@const [lo, hi] = yExtent(p.outline)}
          {@const x = box.x1 + pad * 0.9}
          <line x1={x} y1={lo} x2={x} y2={hi} class="dim" class:bad={!p.fits} style="stroke-width:{px * 1.5}" />
          <line x1={x - px * 5} y1={lo} x2={x + px * 5} y2={lo} class="dim" class:bad={!p.fits} style="stroke-width:{px * 1.5}" />
          <line x1={x - px * 5} y1={hi} x2={x + px * 5} y2={hi} class="dim" class:bad={!p.fits} style="stroke-width:{px * 1.5}" />
          <text x={x + px * 9} y={-(lo + hi) / 2} transform="scale(1 -1)" class="label" class:bad={!p.fits} style="font-size:{px * 13}px" dominant-baseline="middle">
            {pieces.length > 1 ? `${k + 1}: ` : ""}{fmt(p.width)} mm
          </text>
        {/each}
      </g>
    </svg>
    <div class="legend">
      <span>roll direction · fibre → </span>
      {#if fit.error}
        <span class="err">{fit.error}</span>
      {:else if !allFit}
        <span class="err">{pieces.length > 1 ? "A piece is" : "This angle is"} wider than the roll's {fmt(fit.usable)} mm. {pieces.length > 1 ? "Move a cut, add one, or use Auto." : "Split it: Auto, or Add line."}</span>
      {:else if pieces.length > 1}
        <span class="okmsg">Every piece fits. Drag a line to move it, drag an end to tilt it; ↑ ↓ nudge the selected line.</span>
      {:else}
        <span class="okmsg">Fits the roll without a split.</span>
      {/if}
    </div>
  </div>
</div>

<style>
  .split {
    position: absolute;
    inset: 0;
    display: grid;
    grid-template-rows: auto 1fr;
    background: var(--canvas);
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    font-size: 13px;
  }
  .what {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .muted {
    color: var(--muted);
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    display: inline-block;
  }
  .group {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .stepper {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .stepper span {
    min-width: 58px;
    text-align: center;
  }
  button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .overlap input {
    width: 48px;
  }
  .spacer {
    flex: 1;
  }
  .canvas {
    position: relative;
    min-height: 0;
  }
  svg {
    width: 100%;
    height: 100%;
    display: block;
    touch-action: none;
  }
  .piece {
    fill: color-mix(in srgb, var(--c) 18%, transparent);
    stroke: none;
  }
  .piece.bad {
    fill: var(--bad-soft);
  }
  .outline {
    fill: none;
    stroke: var(--ink);
    opacity: 0.5;
  }
  .cut {
    stroke: var(--accent);
    stroke-dasharray: 6 4;
    vector-effect: none;
  }
  .cut.sel {
    stroke-dasharray: none;
  }
  .hit {
    stroke: transparent;
    cursor: ns-resize;
  }
  .handle {
    fill: var(--panel);
    stroke: var(--accent);
    cursor: grab;
  }
  .handle.sel {
    fill: var(--accent);
  }
  .dim {
    stroke: var(--ok);
  }
  .dim.bad,
  .label.bad {
    stroke: var(--bad);
    fill: var(--bad);
  }
  .label {
    fill: var(--ok);
    font-weight: 600;
  }
  .legend {
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: 10px;
    display: flex;
    gap: 12px;
    font-size: 12px;
    color: var(--muted);
    pointer-events: none;
  }
  .err {
    color: var(--bad);
  }
  .okmsg {
    color: var(--ok);
  }
</style>
