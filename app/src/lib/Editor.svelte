<script lang="ts">
  // Large view of one ply. Geometry is drawn in drawing coordinates (mm, y up)
  // inside a single y-flip; overlays (0° axis, highlights, labels) are drawn in
  // screen pixels via toScreen(). toDrawing()/toScreen() are the only two
  // conversions between the systems.
  import type { Edge, Ply, RefPick, XY } from "./types";
  import { angleColor, centre, diag, fmtAngle, hatchD, normAxis, pathD, plyD } from "./geo";

  let {
    ply,
    pointsMode = false,
    preview = 0,
    onpick,
    onpointsdone,
  }: {
    ply: Ply;
    pointsMode?: boolean;
    /** index of the variant whose fibre hatching is shown; -1 = 0° axis only; -2 = all */
    preview?: number;
    onpick: (pick: RefPick) => void;
    onpointsdone: () => void;
  } = $props();

  let host = $state<HTMLDivElement>();
  let size = $state({ w: 800, h: 600 });
  let zoom = $state(1);
  let pan = $state<XY>([0, 0]);
  let mouse = $state<XY | null>(null); // drawing coords
  let mouseScreen = $state<XY>([0, 0]);
  let firstPoint = $state<XY | null>(null);
  let dragging = $state(false);
  let lastId = -1;

  $effect(() => {
    if (ply.id !== lastId) {
      lastId = ply.id;
      zoom = 1;
      pan = [0, 0];
      firstPoint = null;
    }
  });
  $effect(() => {
    if (!pointsMode) firstPoint = null;
  });
  $effect(() => {
    if (!host) return;
    const ro = new ResizeObserver(([e]) => {
      size = { w: Math.max(50, e.contentRect.width), h: Math.max(50, e.contentRect.height) };
    });
    ro.observe(host);
    return () => ro.disconnect();
  });

  // mm per screen pixel at zoom 1: the ply fills ~85% of the view.
  const baseScale = $derived.by(() => {
    const [x0, y0, x1, y1] = ply.bbox;
    return Math.max((x1 - x0) / size.w, (y1 - y0) / size.h, 1e-6) / 0.85;
  });
  const mmPerPx = $derived(baseScale / zoom);
  const vb = $derived.by(() => {
    const [cx, cy] = centre(ply);
    const w = size.w * mmPerPx, h = size.h * mmPerPx;
    // SVG user space is drawing space with y negated.
    return { x: cx + pan[0] - w / 2, y: -(cy + pan[1]) - h / 2, w, h };
  });

  function toDrawing(clientX: number, clientY: number): XY {
    const r = host!.getBoundingClientRect();
    const sx = vb.x + ((clientX - r.left) * vb.w) / size.w;
    const sy = vb.y + ((clientY - r.top) * vb.h) / size.h;
    return [sx, -sy];
  }
  function toScreen(p: XY): XY {
    return [((p[0] - vb.x) * size.w) / vb.w, ((-p[1] - vb.y) * size.h) / vb.h];
  }

  function distToSeg(p: XY, a: XY, b: XY): number {
    const dx = b[0] - a[0], dy = b[1] - a[1];
    const l2 = dx * dx + dy * dy;
    const t = l2 > 0 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2)) : 0;
    return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy));
  }

  const hoverEdge = $derived.by((): Edge | null => {
    if (!mouse || pointsMode || dragging) return null;
    let best: Edge | null = null;
    let bd = 9 * mmPerPx;
    for (const e of ply.edges) {
      const d = distToSeg(mouse, e.a, e.b);
      if (d < bd) {
        bd = d;
        best = e;
      }
    }
    return best;
  });

  /** Nearest vertex (outline, holes, marks) within 10 px, else the point itself. */
  function snap(p: XY): XY {
    let best = p;
    let bd = 10 * mmPerPx;
    const consider = (q: XY) => {
      const d = Math.hypot(q[0] - p[0], q[1] - p[1]);
      if (d < bd) {
        bd = d;
        best = q;
      }
    };
    ply.outline.forEach(consider);
    ply.holes.forEach((h) => h.forEach(consider));
    ply.marks.forEach((m) => m.pts.forEach(consider));
    return best;
  }
  const snapped = $derived(mouse && pointsMode ? snap(mouse) : null);

  // Panning: drag anywhere; a click is a press without movement.
  let press: { x: number; y: number; pan: XY } | null = null;
  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0 && e.button !== 1) return;
    press = { x: e.clientX, y: e.clientY, pan: [...pan] as XY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onpointermove(e: PointerEvent) {
    const r = host!.getBoundingClientRect();
    mouseScreen = [e.clientX - r.left, e.clientY - r.top];
    mouse = toDrawing(e.clientX, e.clientY);
    if (press) {
      const dx = e.clientX - press.x, dy = e.clientY - press.y;
      if (dragging || Math.hypot(dx, dy) > 4) {
        dragging = true;
        pan = [press.pan[0] - dx * mmPerPx, press.pan[1] + dy * mmPerPx];
      }
    }
  }
  function onpointerup(e: PointerEvent) {
    const wasDrag = dragging;
    press = null;
    dragging = false;
    if (wasDrag || e.button !== 0) return;
    const p = toDrawing(e.clientX, e.clientY);
    if (pointsMode) {
      const q = snap(p);
      if (!firstPoint) firstPoint = q;
      else if (Math.hypot(q[0] - firstPoint[0], q[1] - firstPoint[1]) > 1e-6) {
        onpick({ kind: "points", a: { x: firstPoint[0], y: firstPoint[1] }, b: { x: q[0], y: q[1] } });
        firstPoint = null;
        onpointsdone();
      }
      return;
    }
    mouse = p;
    if (hoverEdge) onpick({ kind: "edge", contour: hoverEdge.contour, seg: hoverEdge.seg });
  }
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    const before = toDrawing(e.clientX, e.clientY);
    const f = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015));
    const z = Math.min(200, Math.max(0.2, zoom * f));
    zoom = z;
    // Keep the point under the cursor fixed.
    const r = host!.getBoundingClientRect();
    const [cx, cy] = centre(ply);
    const w = size.w * (baseScale / z), h = size.h * (baseScale / z);
    const nx = before[0] - (e.clientX - r.left) * (w / size.w) + w / 2 - cx;
    const ny = before[1] + (e.clientY - r.top) * (h / size.h) - h / 2 - cy;
    pan = [nx, ny];
  }
  export function fit() {
    zoom = 1;
    pan = [0, 0];
  }

  const gap = $derived(13 * mmPerPx);
  const hatchList = $derived.by(() => {
    if (ply.ref_deg == null || preview === -1) return [];
    const vs = preview === -2 ? ply.variants : ply.variants[preview] ? [ply.variants[preview]] : ply.variants.slice(0, 1);
    if (vs.length === 0) return [{ angle: 0, deg: ply.ref_deg, color: angleColor(0), op: 0.55 }];
    return vs.map((v) => ({ angle: v.angle_deg, deg: ply.ref_deg! + v.angle_deg, color: angleColor(v.angle_deg), op: vs.length > 1 ? 0.45 : 0.6 }));
  });

  // 0° axis in screen space, through the bbox centre.
  const axis = $derived.by(() => {
    if (ply.ref_deg == null) return null;
    const c = centre(ply);
    const t = (ply.ref_deg * Math.PI) / 180;
    const r = diag(ply) * 0.62;
    const a = toScreen([c[0] - Math.cos(t) * r, c[1] - Math.sin(t) * r]);
    const b = toScreen([c[0] + Math.cos(t) * r, c[1] + Math.sin(t) * r]);
    return { a, b, ang: Math.atan2(b[1] - a[1], b[0] - a[0]) };
  });
  function head(p: XY, ang: number, s = 11): string {
    const l = [p[0] - s * Math.cos(ang - 0.4), p[1] - s * Math.sin(ang - 0.4)];
    const r = [p[0] - s * Math.cos(ang + 0.4), p[1] - s * Math.sin(ang + 0.4)];
    return `M${l[0]} ${l[1]}L${p[0]} ${p[1]}L${r[0]} ${r[1]}`;
  }

  const pointsPreview = $derived.by(() => {
    if (!pointsMode || !firstPoint || !snapped) return null;
    const a = toScreen(firstPoint), b = toScreen(snapped);
    const deg = normAxis((Math.atan2(snapped[1] - firstPoint[1], snapped[0] - firstPoint[0]) * 180) / Math.PI);
    return { a, b, deg };
  });

  // Scale bar: a round length near 120 px.
  const scaleBar = $derived.by(() => {
    const target = 120 * mmPerPx;
    const p = Math.pow(10, Math.floor(Math.log10(target)));
    const len = [1, 2, 5, 10].map((m) => m * p).reduce((best, v) => (Math.abs(v - target) < Math.abs(best - target) ? v : best));
    return { len, px: len / mmPerPx };
  });
</script>

<div
  class="editor"
  class:points={pointsMode}
  class:edge-hover={!!hoverEdge}
  class:dragging
  bind:this={host}
  role="application"
  aria-label="Ply editor: click an edge that runs along the 0° direction"
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointerleave={() => (mouse = null)}
  {onwheel}
  ondblclick={fit}
>
  <svg class="geom" width={size.w} height={size.h} viewBox="{vb.x} {vb.y} {vb.w} {vb.h}">
    <defs>
      <clipPath id="ply-clip-{ply.id}" clip-rule="evenodd">
        <path d={plyD(ply)} transform="scale(1 -1)" clip-rule="evenodd" />
      </clipPath>
    </defs>
    <g transform="scale(1 -1)">
      <path d={plyD(ply)} class="fill" fill-rule="evenodd" />
    </g>
    <g clip-path="url(#ply-clip-{ply.id})">
      <g transform="scale(1 -1)">
        {#each hatchList as h (h.angle)}
          <path d={hatchD(ply, h.deg, gap)} stroke={h.color} stroke-opacity={h.op} class="hatch" />
        {/each}
      </g>
    </g>
    <g transform="scale(1 -1)">
      <path d={pathD(ply.outline)} class="outline" />
      {#each ply.holes as h}
        <path d={pathD(h)} class="outline hole" />
      {/each}
      {#each ply.marks as m}
        <path d={pathD(m.pts, m.closed)} class="mark" />
      {/each}
    </g>
  </svg>

  <svg class="overlay" width={size.w} height={size.h}>
    {#if axis}
      <line x1={axis.a[0]} y1={axis.a[1]} x2={axis.b[0]} y2={axis.b[1]} class="axis-halo" />
      <line x1={axis.a[0]} y1={axis.a[1]} x2={axis.b[0]} y2={axis.b[1]} class="axis" />
      <path d={head(axis.b, axis.ang)} class="axis" />
      <path d={head(axis.a, axis.ang + Math.PI)} class="axis" />
      <text x={axis.b[0] + 14 * Math.cos(axis.ang)} y={axis.b[1] + 14 * Math.sin(axis.ang) + 4} class="axis-label">0°</text>
    {/if}
    {#if hoverEdge}
      {@const a = toScreen(hoverEdge.a)}
      {@const b = toScreen(hoverEdge.b)}
      <line x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} class="edge-hl" />
    {/if}
    {#if pointsMode && firstPoint}
      {@const a = toScreen(firstPoint)}
      <circle cx={a[0]} cy={a[1]} r="5" class="pt" />
    {/if}
    {#if pointsPreview}
      <line x1={pointsPreview.a[0]} y1={pointsPreview.a[1]} x2={pointsPreview.b[0]} y2={pointsPreview.b[1]} class="edge-hl" />
    {/if}
    {#if pointsMode && snapped}
      {@const s = toScreen(snapped)}
      <circle cx={s[0]} cy={s[1]} r="4" class="pt ghost" />
    {/if}
    <!-- drawing axes compass -->
    <g transform="translate(22 {size.h - 22})" class="compass">
      <line x1="0" y1="0" x2="34" y2="0" /><path d="M28 -4L34 0L28 4" />
      <line x1="0" y1="0" x2="0" y2="-34" /><path d="M-4 -28L0 -34L4 -28" />
      <text x="38" y="4">X</text><text x="-4" y="-40">Y</text>
    </g>
    <g transform="translate({size.w - 22 - scaleBar.px} {size.h - 20})" class="scalebar">
      <line x1="0" y1="0" x2={scaleBar.px} y2="0" />
      <line x1="0" y1="-4" x2="0" y2="4" /><line x1={scaleBar.px} y1="-4" x2={scaleBar.px} y2="4" />
      <text x={scaleBar.px / 2} y="-7">{scaleBar.len >= 1 ? scaleBar.len : scaleBar.len.toPrecision(1)} mm</text>
    </g>
  </svg>

  {#if hoverEdge}
    <div class="tip" style="left:{mouseScreen[0] + 14}px; top:{mouseScreen[1] + 14}px">
      <b>{fmtAngle(hoverEdge.angle)}</b> · {hoverEdge.len.toFixed(1)} mm{hoverEdge.contour > 0 ? " · hole edge" : ""}<br />
      <span>click: this edge is 0°</span>
    </div>
  {:else if pointsPreview}
    <div class="tip" style="left:{mouseScreen[0] + 14}px; top:{mouseScreen[1] + 14}px">
      <b>{fmtAngle(pointsPreview.deg)}</b><br /><span>click the second point</span>
    </div>
  {:else if pointsMode && !firstPoint && mouse}
    <div class="tip" style="left:{mouseScreen[0] + 14}px; top:{mouseScreen[1] + 14}px"><span>click the first point (snaps to corners)</span></div>
  {/if}
</div>

<style>
  .editor {
    position: relative;
    width: 100%;
    height: 100%;
    overflow: hidden;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }
  .editor.edge-hover {
    cursor: pointer;
  }
  .editor.points {
    cursor: crosshair;
  }
  .editor.dragging {
    cursor: grabbing;
  }
  svg {
    position: absolute;
    inset: 0;
    display: block;
  }
  .overlay {
    pointer-events: none;
  }
  .fill {
    fill: var(--ply-fill);
  }
  .hatch {
    fill: none;
    stroke-width: 1.6px;
    vector-effect: non-scaling-stroke;
  }
  .outline {
    fill: none;
    stroke: var(--ink);
    stroke-width: 1.6px;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }
  .mark {
    fill: none;
    stroke: var(--mark);
    stroke-width: 1.2px;
    stroke-dasharray: 5 3;
    vector-effect: non-scaling-stroke;
  }
  .axis,
  .axis-halo {
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .axis {
    stroke: var(--axis);
    stroke-width: 2.2px;
    stroke-dasharray: 10 5;
  }
  path.axis {
    stroke-dasharray: none;
  }
  .axis-halo {
    stroke: var(--canvas);
    stroke-width: 6px;
    opacity: 0.8;
  }
  .axis-label {
    fill: var(--axis);
    font: 600 13px var(--font);
    text-anchor: middle;
    paint-order: stroke;
    stroke: var(--canvas);
    stroke-width: 4px;
  }
  .edge-hl {
    stroke: var(--accent);
    stroke-width: 5px;
    stroke-linecap: round;
    opacity: 0.85;
  }
  .pt {
    fill: var(--accent);
    stroke: var(--canvas);
    stroke-width: 2px;
  }
  .pt.ghost {
    fill: none;
    stroke: var(--accent);
  }
  .compass line,
  .compass path,
  .scalebar line {
    stroke: var(--muted);
    stroke-width: 1.4px;
    fill: none;
  }
  .compass text,
  .scalebar text {
    fill: var(--muted);
    font: 11px var(--font);
  }
  .scalebar text {
    text-anchor: middle;
  }
  .tip {
    position: absolute;
    pointer-events: none;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 12px;
    line-height: 1.35;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.12);
    white-space: nowrap;
  }
  .tip span {
    color: var(--muted);
  }
</style>
