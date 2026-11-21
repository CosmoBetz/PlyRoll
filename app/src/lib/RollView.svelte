<script lang="ts">
  // A nest on the roll, seen from above like the cutter sees it: x runs along
  // the roll, y across it, y up (one flip, as in the editor), so the drawing
  // face matches the cut file. Plies are coloured by ply angle and hatched
  // along their fibre direction on the roll.
  import type { LayoutShapes, Placement, XY } from "./types";
  import { angleColor, fmtAngle, pathD } from "./geo";

  let {
    shapes,
    placements,
    usedLength,
    live = false,
  }: {
    shapes: LayoutShapes;
    placements: Placement[];
    usedLength: number | null;
    /** Still optimising: drawn lighter, no hover. */
    live?: boolean;
  } = $props();

  let host = $state<HTMLDivElement>();
  let size = $state({ w: 800, h: 400 });
  let zoom = $state(1);
  let pan = $state<XY>([0, 0]);
  let mouse = $state<XY | null>(null);
  let mouseScreen = $state<XY>([0, 0]);
  let dragging = $state(false);

  $effect(() => {
    if (!host) return;
    const ro = new ResizeObserver(([e]) => {
      size = { w: Math.max(50, e.contentRect.width), h: Math.max(50, e.contentRect.height) };
    });
    ro.observe(host);
    return () => ro.disconnect();
  });

  const m = $derived(shapes.material);

  type Placed = { pl: Placement; pts: XY[]; holes: XY[][]; bbox: [number, number, number, number]; color: string };
  const placed = $derived.by((): Placed[] =>
    placements.map((pl) => {
      const t = (pl.rotation * Math.PI) / 180;
      const c = Math.cos(t), s = Math.sin(t);
      const f = ([x, y]: XY): XY => [x * c - y * s + pl.tx, x * s + y * c + pl.ty];
      const g = shapes.shapes[pl.shape];
      const pts = g.outline.map(f);
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (const [x, y] of pts) {
        x0 = Math.min(x0, x);
        y0 = Math.min(y0, y);
        x1 = Math.max(x1, x);
        y1 = Math.max(y1, y);
      }
      return { pl, pts, holes: g.holes.map((h) => h.map(f)), bbox: [x0, y0, x1, y1], color: angleColor(pl.angle) };
    }),
  );

  /** Length shown: the used length, or how far the plies reach while running. */
  const extent = $derived(Math.max(usedLength ?? 0, m.lead_trim, ...placed.map((p) => p.bbox[2])) || m.roll_width);
  const world = $derived({ x0: -m.roll_width * 0.03, x1: extent + m.roll_width * 0.12, y0: 0, y1: m.roll_width });

  const baseScale = $derived(Math.max((world.x1 - world.x0) / size.w, (world.y1 - world.y0) / (size.h - 40), 1e-6) / 0.96);
  const mmPerPx = $derived(baseScale / zoom);
  const vb = $derived.by(() => {
    const cx = (world.x0 + world.x1) / 2, cy = (world.y0 + world.y1) / 2;
    const w = size.w * mmPerPx, h = size.h * mmPerPx;
    return { x: cx + pan[0] - w / 2, y: -(cy + pan[1]) - h / 2, w, h };
  });

  function toRoll(clientX: number, clientY: number): XY {
    const r = host!.getBoundingClientRect();
    return [vb.x + ((clientX - r.left) * vb.w) / size.w, -(vb.y + ((clientY - r.top) * vb.h) / size.h)];
  }
  function toScreen(p: XY): XY {
    return [((p[0] - vb.x) * size.w) / vb.w, ((-p[1] - vb.y) * size.h) / vb.h];
  }

  function inside(p: XY, poly: XY[]): boolean {
    let c = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [xi, yi] = poly[i], [xj, yj] = poly[j];
      if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) c = !c;
    }
    return c;
  }
  const hover = $derived.by(() => {
    if (!mouse || dragging || live) return null;
    const p = mouse;
    return placed.find((q) => p[0] >= q.bbox[0] && p[0] <= q.bbox[2] && p[1] >= q.bbox[1] && p[1] <= q.bbox[3] && inside(p, q.pts)) ?? null;
  });

  let press: { x: number; y: number; pan: XY } | null = null;
  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0 && e.button !== 1) return;
    press = { x: e.clientX, y: e.clientY, pan: [...pan] as XY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onpointermove(e: PointerEvent) {
    const r = host!.getBoundingClientRect();
    mouseScreen = [e.clientX - r.left, e.clientY - r.top];
    mouse = toRoll(e.clientX, e.clientY);
    if (press) {
      const dx = e.clientX - press.x, dy = e.clientY - press.y;
      if (dragging || Math.hypot(dx, dy) > 4) {
        dragging = true;
        pan = [press.pan[0] - dx * mmPerPx, press.pan[1] + dy * mmPerPx];
      }
    }
  }
  function onpointerup() {
    press = null;
    dragging = false;
  }
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    // Horizontal trackpad scrolling pans along the roll.
    if (!e.ctrlKey && Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      pan = [pan[0] + e.deltaX * mmPerPx, pan[1]];
      return;
    }
    const before = toRoll(e.clientX, e.clientY);
    const z = Math.min(100, Math.max(0.5, zoom * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015))));
    zoom = z;
    const r = host!.getBoundingClientRect();
    const cx = (world.x0 + world.x1) / 2, cy = (world.y0 + world.y1) / 2;
    const w = size.w * (baseScale / z), h = size.h * (baseScale / z);
    pan = [before[0] - (e.clientX - r.left) * (w / size.w) + w / 2 - cx, before[1] + (e.clientY - r.top) * (h / size.h) - h / 2 - cy];
  }
  export function fit() {
    zoom = 1;
    pan = [0, 0];
  }

  /** Fibre lines along `deg` through a placed ply's bbox, `gap` apart (roll mm). */
  function hatch(q: Placed, deg: number, gap: number): string {
    const [x0, y0, x1, y1] = q.bbox;
    const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
    const r = Math.hypot(x1 - x0, y1 - y0) / 2 + gap;
    const t = (deg * Math.PI) / 180;
    const ux = Math.cos(t), uy = Math.sin(t);
    const n = Math.ceil(r / gap);
    let d = "";
    for (let i = -n; i <= n; i++) {
      const ox = cx - uy * i * gap, oy = cy + ux * i * gap;
      d += `M${ox - ux * r} ${oy - uy * r}L${ox + ux * r} ${oy + uy * r}`;
    }
    return d;
  }
  const gap = $derived(8 * mmPerPx);
  const showHatch = $derived(mmPerPx < 4);

  const labels = $derived.by(() =>
    placed
      .map((q) => {
        const a = toScreen([q.bbox[0], q.bbox[3]]), b = toScreen([q.bbox[2], q.bbox[1]]);
        const w = b[0] - a[0], h = b[1] - a[1];
        return { q, x: (a[0] + b[0]) / 2, y: (a[1] + b[1]) / 2, w, h };
      })
      .filter((l) => l.w > 56 && l.h > 22 && l.x > -100 && l.x < size.w + 100),
  );

  const usedX = $derived(usedLength != null ? toScreen([usedLength, 0]) : null);
  const scaleBar = $derived.by(() => {
    const target = 120 * mmPerPx;
    const p = Math.pow(10, Math.floor(Math.log10(target)));
    const len = [1, 2, 5, 10].map((k) => k * p).reduce((best, v) => (Math.abs(v - target) < Math.abs(best - target) ? v : best));
    return { len, px: len / mmPerPx };
  });
  const rollEnd = $derived(world.x1);
</script>

<div
  class="roll"
  class:dragging
  class:live
  bind:this={host}
  role="img"
  aria-label="Nested plies on the roll"
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointerleave={() => (mouse = null)}
  {onwheel}
  ondblclick={fit}
>
  <svg width={size.w} height={size.h} viewBox="{vb.x} {vb.y} {vb.w} {vb.h}">
    <defs>
      {#if showHatch}
        {#each placed as q, i}
          <clipPath id="roll-clip-{i}"><path d={pathD(q.pts) + q.holes.map((h) => pathD(h)).join("")} clip-rule="evenodd" /></clipPath>
        {/each}
      {/if}
    </defs>
    <g transform="scale(1 -1)">
      <rect x={0} y={0} width={rollEnd} height={m.roll_width} class="band" />
      <rect x={0} y={0} width={rollEnd} height={m.edge_trim} class="trim" />
      <rect x={0} y={m.roll_width - m.edge_trim} width={rollEnd} height={m.edge_trim} class="trim" />
      <rect x={0} y={0} width={m.lead_trim} height={m.roll_width} class="trim" />
      {#each placed as q, i}
        <path d={pathD(q.pts) + q.holes.map((h) => pathD(h)).join("")} fill={q.color} class="ply" class:hot={hover === q} fill-rule="evenodd" />
        {#if showHatch}
          <path d={hatch(q, q.pl.fibre, gap)} stroke={q.color} clip-path="url(#roll-clip-{i})" class="hatch" />
        {/if}
        <path d={pathD(q.pts)} class="edge" stroke={q.color} />
        {#each q.holes as h}<path d={pathD(h)} class="edge" stroke={q.color} />{/each}
      {/each}
    </g>
  </svg>

  <svg class="overlay" width={size.w} height={size.h}>
    {#each labels as l (l.q)}
      <text x={l.x} y={l.y} class="label">{l.q.pl.label}</text>
    {/each}
    {#if usedX}
      <line x1={usedX[0]} y1={toScreen([0, m.roll_width + 30 * mmPerPx])[1]} x2={usedX[0]} y2={toScreen([0, -12 * mmPerPx])[1]} class="cut" />
      <text x={usedX[0] - 6} y={toScreen([0, m.roll_width])[1] - 8} class="cut-label">used {usedLength!.toFixed(1)} mm</text>
    {/if}
    <g transform="translate({size.w - scaleBar.px - 24},{size.h - 22})">
      <line x1="0" y1="0" x2={scaleBar.px} y2="0" class="bar" />
      <line x1="0" y1="-4" x2="0" y2="4" class="bar" />
      <line x1={scaleBar.px} y1="-4" x2={scaleBar.px} y2="4" class="bar" />
      <text x={scaleBar.px / 2} y="-7" class="bar-label">{scaleBar.len >= 1000 ? `${scaleBar.len / 1000} m` : `${scaleBar.len} mm`}</text>
    </g>
    <g transform="translate(18,{size.h - 20})">
      <line x1="0" y1="0" x2="34" y2="0" class="bar" />
      <path d="M28 -4L34 0L28 4" class="bar" />
      <text x="40" y="4" class="bar-label start">roll direction · 0° fibre</text>
    </g>
  </svg>

  {#if hover}
    <div class="tip" style="left:{mouseScreen[0] + 14}px; top:{mouseScreen[1] + 14}px">
      <b>{hover.pl.label}</b> · copy {hover.pl.copy}
      {#if hover.pl.kit}<br />kit {hover.pl.kit}{/if}
      <br /><span class="muted">turned {fmtAngle(hover.pl.rotation)} · fibre {fmtAngle(hover.pl.fibre)} on the roll</span>
    </div>
  {/if}
</div>

<style>
  .roll {
    position: absolute;
    inset: 0;
    overflow: hidden;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .roll.dragging {
    cursor: grabbing;
  }
  svg {
    position: absolute;
    inset: 0;
  }
  .overlay {
    pointer-events: none;
  }
  .band {
    fill: var(--panel);
    stroke: var(--line-strong);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .trim {
    fill: var(--line);
    opacity: 0.55;
  }
  .ply {
    fill-opacity: 0.16;
  }
  .ply.hot {
    fill-opacity: 0.4;
  }
  .live .ply {
    fill-opacity: 0.1;
  }
  .hatch {
    stroke-width: 1;
    stroke-opacity: 0.45;
    vector-effect: non-scaling-stroke;
    fill: none;
  }
  .edge {
    fill: none;
    stroke-width: 1.4;
    vector-effect: non-scaling-stroke;
  }
  .label {
    font-size: 11px;
    fill: var(--ink);
    text-anchor: middle;
    dominant-baseline: middle;
    paint-order: stroke;
    stroke: var(--panel);
    stroke-width: 3px;
  }
  .cut {
    stroke: var(--ink);
    stroke-width: 1.5;
    stroke-dasharray: 6 4;
  }
  .cut-label {
    text-anchor: end;
    font-size: 12px;
    font-weight: 600;
    fill: var(--ink);
  }
  .bar {
    stroke: var(--muted);
    stroke-width: 1.5;
    fill: none;
  }
  .bar-label {
    font-size: 11px;
    fill: var(--muted);
    text-anchor: middle;
  }
  .bar-label.start {
    text-anchor: start;
  }
  .tip {
    position: absolute;
    pointer-events: none;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 12px;
    line-height: 1.45;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.12);
    white-space: nowrap;
  }
  .muted {
    color: var(--muted);
  }
</style>
