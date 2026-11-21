<script lang="ts">
  import type { Ply } from "./types";
  import { angleColor, fmtAngle, hatchD, pathD, plyD } from "./geo";

  let { ply, selected = false, active = false, onselect }: { ply: Ply; selected?: boolean; active?: boolean; onselect: (e: MouseEvent) => void } =
    $props();

  const vb = $derived.by(() => {
    const [x0, y0, x1, y1] = ply.bbox;
    const w = x1 - x0, h = y1 - y0;
    const pad = Math.max(w, h) * 0.08;
    return { x: x0 - pad, y: -y1 - pad, w: w + 2 * pad, h: h + 2 * pad, unit: Math.max(w, h) / 110 };
  });
  const STATUS_TEXT = { unset: "no 0°", check: "check 0°", ok: "0° set" };
</script>

<button class="thumb" class:selected class:active onclick={onselect} title="{ply.name} — {ply.file}{ply.note ? '\n' + ply.note : ''}">
  <svg viewBox="{vb.x} {vb.y} {vb.w} {vb.h}" preserveAspectRatio="xMidYMid meet" aria-hidden="true">
    <defs>
      <clipPath id="thumb-clip-{ply.id}"><path d={plyD(ply)} transform="scale(1 -1)" clip-rule="evenodd" /></clipPath>
    </defs>
    <g transform="scale(1 -1)">
      <path d={plyD(ply)} fill-rule="evenodd" class="fill" />
    </g>
    {#if ply.ref_deg != null}
      <g clip-path="url(#thumb-clip-{ply.id})">
        <path d={hatchD(ply, ply.ref_deg, vb.unit * 7)} transform="scale(1 -1)" stroke={angleColor(0)} class="hatch" />
      </g>
    {/if}
    <g transform="scale(1 -1)">
      <path d={pathD(ply.outline)} class="outline" />
      {#each ply.holes as h}<path d={pathD(h)} class="outline" />{/each}
    </g>
  </svg>
  <span class="name">{ply.name}</span>
  <span class="meta">
    <span class="badge {ply.status}">{STATUS_TEXT[ply.status]}</span>
    {#if ply.ref_deg != null}<span class="deg">{fmtAngle(ply.ref_deg)}</span>{/if}
    <span class="qty" class:zero={ply.total_qty === 0}>×{ply.total_qty}</span>
  </span>
</button>

<style>
  .thumb {
    display: grid;
    grid-template-rows: 92px auto auto;
    gap: 3px;
    padding: 6px 6px 7px;
    border: 1.5px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
    text-align: left;
    cursor: pointer;
    min-width: 0;
    font: inherit;
    color: inherit;
  }
  .thumb:hover {
    border-color: var(--line-strong);
  }
  .thumb.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .thumb.active {
    box-shadow: 0 0 0 2px var(--accent);
  }
  svg {
    width: 100%;
    height: 92px;
    background: var(--canvas);
    border-radius: 5px;
  }
  .fill {
    fill: var(--ply-fill);
  }
  .hatch {
    fill: none;
    stroke-width: 1px;
    vector-effect: non-scaling-stroke;
    opacity: 0.7;
  }
  .outline {
    fill: none;
    stroke: var(--ink);
    stroke-width: 1.1px;
    vector-effect: non-scaling-stroke;
  }
  .name {
    font-size: 12px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    color: var(--muted);
  }
  .qty {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  .qty.zero {
    color: var(--warn);
  }
</style>
