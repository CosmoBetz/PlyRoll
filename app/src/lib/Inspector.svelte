<script lang="ts">
  import type { Ply, RefPick, Variant } from "./types";
  import { angleColor, fmtAngle, ORIGIN_LABEL, totalQty } from "./geo";

  let {
    active,
    selected,
    pointsMode,
    onpick,
    ontogglepoints,
    onvariants,
    oncosmetic,
    onmirrored,
    onrename,
    onremove,
  }: {
    active: Ply;
    selected: Ply[];
    pointsMode: boolean;
    onpick: (pick: RefPick) => void;
    ontogglepoints: () => void;
    onvariants: (vs: Variant[]) => void;
    oncosmetic: (on: boolean) => void;
    onmirrored: (on: boolean) => void;
    onrename: (name: string) => void;
    onremove: () => void;
  } = $props();

  const multi = $derived(selected.length > 1);
  const allCosmetic = $derived(selected.every((p) => p.cosmetic));
  const someCosmetic = $derived(selected.some((p) => p.cosmetic));
  const allMirrored = $derived(selected.every((p) => p.mirrored));
  const someMirrored = $derived(selected.some((p) => p.mirrored));
  const key = (vs: Variant[]) => JSON.stringify(vs.map((v) => [v.angle_deg, v.qty, v.kit ?? ""]));
  const variantsDiffer = $derived(multi && selected.some((p) => key(p.variants) !== key(active.variants)));
  const anyLayer = $derived(selected.some((p) => p.layer_ref_deg != null));
  const toConfirm = $derived(selected.filter((p) => p.status === "check").length);

  let angleText = $state("");
  let shownAngle = ""; // what the field was filled with; only a real edit is applied
  let nameText = $state("");
  $effect(() => {
    angleText = shownAngle = active.ref_deg != null ? String(Math.round(active.ref_deg * 1000) / 1000) : "";
  });
  $effect(() => {
    nameText = active.name;
  });

  function applyAngle() {
    if (angleText.trim() === shownAngle) return;
    const t = angleText.trim().replace("−", "-").replace("°", "");
    if (t === "") return;
    const deg = Number(t);
    if (Number.isFinite(deg)) onpick({ kind: "angle", deg });
    else angleText = shownAngle;
  }

  // Variant editing works on a copy of the active ply's list; every change is sent.
  function update(f: (vs: Variant[]) => Variant[]) {
    onvariants(f(active.variants.map((v) => ({ ...v }))));
  }
  function addAngle(angle: number, qty = 1) {
    update((vs) => {
      const hit = vs.find((v) => v.angle_deg === angle);
      if (hit) hit.qty += qty;
      else vs.push({ angle_deg: angle, qty, kit: vs[vs.length - 1]?.kit ?? null });
      return vs;
    });
  }
  function setField(i: number, field: "angle_deg" | "qty" | "kit", raw: string) {
    update((vs) => {
      const v = vs[i];
      if (field === "kit") v.kit = raw.trim() || null;
      else {
        const n = Number(raw.trim().replace("−", "-").replace("°", ""));
        if (!Number.isFinite(n)) return vs;
        if (field === "qty") v.qty = Math.max(0, Math.round(n));
        else v.angle_deg = n;
      }
      return vs;
    });
  }
  let isoQty = $state(1);
</script>

<aside class="inspector">
  <section>
    {#if multi}
      <h2>{selected.length} plies selected</h2>
      <p class="hint">Rules (X, Y, edge, box, layer, angle, turn) and the settings below apply to all of them. Edge and two-point picks use the large view: {active.name}.</p>
    {:else}
      <input
        class="name"
        bind:value={nameText}
        onkeydown={(e) => {
          if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
          if (e.key === "Escape") {
            nameText = active.name;
            (e.currentTarget as HTMLInputElement).blur();
          }
        }}
        onblur={() => nameText.trim() && nameText !== active.name && onrename(nameText)}
        aria-label="Ply name"
      />
      <p class="src">{active.file}{active.part !== active.file.replace(/\.dxf$/i, "") ? ` · part ${active.part}` : ""}</p>
    {/if}
  </section>

  <section>
    <h3>0° reference</h3>
    {#if !multi}
      <div class="status {active.status}">
        {#if active.status === "unset"}
          <b>No 0° reference.</b> Click an edge along 0° in the view, or use a rule below.
        {:else if active.status === "check"}
          <b>Check:</b> {active.note ?? "confirm the 0° reference"}
        {:else}
          0° at <b>{fmtAngle(active.ref_deg!)}</b> from drawing X <span>({ORIGIN_LABEL[active.ref_origin ?? "manual"]})</span>
        {/if}
      </div>
    {/if}
    {#if toConfirm > 0}
      <button class="primary wide" onclick={() => onpick({ kind: "confirm" })}>Confirm 0° {multi ? `on ${toConfirm} plies` : "as shown"} <kbd>↵</kbd></button>
    {/if}
    <div class="row">
      <label class="angle">
        <span>Angle</span>
        <input
          bind:value={angleText}
          placeholder="e.g. 30"
          inputmode="decimal"
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.stopPropagation();
              applyAngle();
            }
          }}
          onblur={applyAngle}
        />
        <span class="unit">°</span>
      </label>
      <div class="turn" role="group" aria-label="Turn the 0° reference">
        <button onclick={() => onpick({ kind: "rotate", delta: -15 })} title="Turn −15° (⇧[ or ⇧Q)">−15</button>
        <button onclick={() => onpick({ kind: "rotate", delta: -1 })} title="Turn −1° ([ or Q)">−1</button>
        <button onclick={() => onpick({ kind: "rotate", delta: 1 })} title="Turn +1° (] or E)">+1</button>
        <button onclick={() => onpick({ kind: "rotate", delta: 15 })} title="Turn +15° (⇧] or ⇧E)">+15</button>
      </div>
    </div>
    <div class="rules">
      <button onclick={() => onpick({ kind: "drawing-x" })}>Drawing X <kbd>X</kbd></button>
      <button onclick={() => onpick({ kind: "drawing-y" })}>Drawing Y <kbd>Y</kbd></button>
      <button onclick={() => onpick({ kind: "longest-edge" })}>Longest edge <kbd>L</kbd></button>
      <button onclick={() => onpick({ kind: "bounding-box" })}>Bounding box <kbd>B</kbd></button>
      <button onclick={() => onpick({ kind: "layer" })} disabled={!anyLayer} title={anyLayer ? "Line on a FIBRE / 0DEG layer" : "No fibre-layer line in this DXF"}>
        Fibre layer <kbd>F</kbd>
      </button>
      <button class:on={pointsMode} onclick={ontogglepoints} title="Click two points in the view">Two points <kbd>P</kbd></button>
    </div>
  </section>

  <section>
    <h3>Ply angles <span class="count">{totalQty(active.variants)} pcs</span></h3>
    {#if multi}
      <p class="hint">{variantsDiffer ? "Selected plies have different angles; editing sets this list on all of them." : "Editing sets this list on all selected plies."}</p>
    {/if}
    {#if active.variants.length === 0}
      <p class="hint">No angles yet. Add the orientations this ply is cut at.</p>
    {:else}
      <table class="variants">
        <thead><tr><th>Angle</th><th>Qty</th><th>Kit</th><th></th></tr></thead>
        <tbody>
          {#each active.variants as v, i (i + ":" + v.angle_deg)}
            <tr>
              <td class="ang">
                <span class="swatch" style="background:{angleColor(v.angle_deg)}"></span>
                <input value={v.angle_deg} inputmode="decimal" onchange={(e) => setField(i, "angle_deg", e.currentTarget.value)} aria-label="Ply angle" />
              </td>
              <td><input value={v.qty} inputmode="numeric" onchange={(e) => setField(i, "qty", e.currentTarget.value)} aria-label="Quantity" /></td>
              <td><input value={v.kit ?? ""} placeholder="—" onchange={(e) => setField(i, "kit", e.currentTarget.value)} aria-label="Kit" /></td>
              <td><button class="icon" title="Remove" onclick={() => update((vs) => vs.filter((_, j) => j !== i))}>×</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    <div class="quick">
      <span>Add</span>
      <button onclick={() => addAngle(0)}>0°</button>
      <button onclick={() => addAngle(45)}>+45°</button>
      <button onclick={() => addAngle(-45)}>−45°</button>
      <button onclick={() => addAngle(90)}>90°</button>
    </div>
    <div class="quick">
      <button onclick={() => update(() => [0, 45, -45, 90].map((a) => ({ angle_deg: a, qty: isoQty, kit: active.variants[0]?.kit ?? null })))}>
        Quasi-iso set
      </button>
      <label class="iso">× <input type="number" min="1" bind:value={isoQty} aria-label="Quasi-iso quantity per angle" /></label>
    </div>
    <p class="hint">Ply angle is measured from the 0° reference, counter-clockwise positive, seen on the side facing up on the roll.</p>
  </section>

  <section>
    <h3>Options</h3>
    <label class="check">
      <input type="checkbox" checked={allCosmetic} indeterminate={someCosmetic && !allCosmetic} onchange={(e) => oncosmetic(e.currentTarget.checked)} />
      <span><b>Cosmetic ply</b><br />Twill warp and weft are never swapped.</span>
    </label>
    <label class="check">
      <input type="checkbox" checked={allMirrored} indeterminate={someMirrored && !allMirrored} onchange={(e) => onmirrored(e.currentTarget.checked)} />
      <span><b>Mirror drawing</b><br />Only when CAD shows the other face. Plies are never flipped on the roll.</span>
    </label>
  </section>

  <section class="danger">
    <button class="ghost" onclick={onremove}>Remove {multi ? `${selected.length} plies` : "ply"} from project</button>
  </section>
</aside>

<style>
  .inspector {
    overflow-y: auto;
    border-left: 1px solid var(--line);
    background: var(--panel);
    font-size: 13px;
  }
  section {
    padding: 12px 14px;
    border-bottom: 1px solid var(--line);
  }
  h2 {
    font-size: 15px;
    margin: 0 0 4px;
  }
  h3 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
    margin: 0 0 8px;
    display: flex;
    justify-content: space-between;
  }
  .count {
    text-transform: none;
    letter-spacing: 0;
  }
  .name {
    width: 100%;
    font-size: 15px;
    font-weight: 600;
    border: 1px solid transparent;
    background: transparent;
    padding: 3px 5px;
    margin-left: -6px;
  }
  .name:hover,
  .name:focus {
    border-color: var(--line);
    background: var(--field);
  }
  .src {
    margin: 2px 0 0;
    color: var(--muted);
    font-size: 12px;
    word-break: break-all;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
    margin: 6px 0;
    line-height: 1.4;
  }
  .status {
    border-radius: 6px;
    padding: 7px 9px;
    margin-bottom: 8px;
    line-height: 1.4;
  }
  .status.unset {
    background: var(--bad-soft);
  }
  .status.check {
    background: var(--warn-soft);
  }
  .status.ok {
    background: var(--ok-soft);
  }
  .status span {
    color: var(--muted);
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 8px 0;
  }
  .angle {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .angle span:first-child {
    color: var(--muted);
  }
  .angle input {
    width: 64px;
    text-align: right;
  }
  .turn {
    display: flex;
    margin-left: auto;
  }
  .turn button {
    border-radius: 0;
    padding: 4px 7px;
    margin-left: -1px;
  }
  .turn button:first-child {
    border-radius: 5px 0 0 5px;
  }
  .turn button:last-child {
    border-radius: 0 5px 5px 0;
  }
  .rules {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 5px;
  }
  .rules button {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .rules button.on {
    background: var(--accent);
    color: white;
    border-color: var(--accent);
  }
  .wide {
    width: 100%;
    margin-bottom: 8px;
    display: flex;
    justify-content: center;
    gap: 8px;
  }
  .variants {
    width: 100%;
    border-collapse: collapse;
  }
  .variants th {
    font-weight: 500;
    color: var(--muted);
    font-size: 11px;
    text-align: left;
    padding: 0 3px 3px;
  }
  .variants td {
    padding: 2px 3px;
  }
  .variants input {
    width: 100%;
    min-width: 0;
  }
  .variants td:nth-child(2) {
    width: 54px;
  }
  .ang {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    flex: none;
  }
  .icon {
    padding: 2px 7px;
    border-color: transparent;
    background: transparent;
    color: var(--muted);
  }
  .icon:hover {
    color: var(--bad);
  }
  .quick {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 8px;
  }
  .quick > span {
    color: var(--muted);
    margin-right: 2px;
  }
  .iso input {
    width: 48px;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 6px 0 10px;
    line-height: 1.35;
  }
  .check span {
    color: var(--muted);
    font-size: 12px;
  }
  .check b {
    color: var(--ink);
    font-size: 13px;
    font-weight: 500;
  }
  .check input {
    margin-top: 3px;
  }
  .danger {
    border-bottom: none;
  }
  .ghost {
    color: var(--bad);
    background: transparent;
  }
</style>
