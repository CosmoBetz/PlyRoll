<script lang="ts">
  import type { AppView, JobSettings, LibraryView, LiveNest as Live, Material } from "./types";
  import RollView from "./RollView.svelte";

  let {
    view,
    library,
    live,
    onjob,
    onstart,
    onstop,
    onexport,
    onmanage,
    onaddmaterial,
    onplies,
  }: {
    view: AppView;
    library: LibraryView;
    live: Live | null;
    onjob: (job: JobSettings) => void;
    onstart: () => void;
    onstop: () => void;
    onexport: () => void;
    onmanage: () => void;
    onaddmaterial: (m: Material) => void;
    onplies: () => void;
  } = $props();

  let roll = $state<RollView>();
  const job = $derived(view.job);
  const mat = $derived(job.material);
  const libEntry = $derived(mat ? library.materials.find((m) => m.name === mat.name) : undefined);
  const libDiffers = $derived(!!libEntry && JSON.stringify(libEntry) !== JSON.stringify(mat));
  const running = $derived(live != null || view.running != null);
  const result = $derived(view.result);

  const PRESETS = [
    { s: 30, label: "30 s" },
    { s: 120, label: "2 min" },
    { s: 600, label: "10 min" },
  ];

  function pickMaterial(name: string) {
    const m = library.materials.find((x) => x.name === name);
    if (m) onjob({ ...job, material: structuredClone($state.snapshot(m)) });
  }
  function setTime(s: number) {
    if (Number.isFinite(s) && s >= 1 && s <= 3600 && s !== job.time_s) onjob({ ...job, time_s: Math.round(s) });
  }
  function setSets(n: number) {
    if (Number.isInteger(n) && n >= 1 && n <= 1000 && n !== job.sets) onjob({ ...job, sets: n });
  }
  const fmtTime = (s: number) => (s >= 60 ? `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")} min` : `${s.toFixed(0)} s`);
  const pct = (x: number) => `${(100 * x).toFixed(1)}%`;

  // What the canvas shows: the running nest, else the stored result.
  const canvas = $derived.by(() => {
    if (live) return { shapes: live.shapes, placements: live.placements ?? [], used: live.placements ? live.length : null, live: true };
    if (result) return { shapes: result.shapes, placements: result.placements, used: result.used_length, live: false };
    return null;
  });
  const exportBlock = $derived(
    !result ? "Nothing nested yet" : !result.fresh ? "Out of date: nest again first" : !result.summary.verification.ok ? "Failed verification" : null,
  );
</script>

<div class="nest">
  <aside class="job">
    <section>
      <h4>Material</h4>
      {#if library.materials.length === 0 && !mat}
        <p class="hint">Your material library is empty.</p>
        <button class="primary" onclick={onmanage}>Add a material…</button>
      {:else}
        <select value={mat?.name ?? ""} onchange={(e) => pickMaterial((e.currentTarget as HTMLSelectElement).value)} disabled={running}>
          {#if !mat}<option value="" disabled>Choose a material…</option>{/if}
          {#if mat && !libEntry}<option value={mat.name}>{mat.name} (project copy)</option>{/if}
          {#each library.materials as m (m.name)}
            <option value={m.name}>{m.name}</option>
          {/each}
        </select>
        {#if mat}
          <dl class="facts">
            <dt>Type</dt><dd>{mat.kind === "ud" ? "UD" : mat.warp_weft_interchangeable ? "Twill, warp/weft swappable" : "Twill"}</dd>
            <dt>Roll</dt><dd>{mat.roll_width} mm, usable {mat.roll_width - 2 * mat.edge_trim} mm</dd>
            <dt>Trims</dt><dd>{mat.edge_trim} mm edges · {mat.lead_trim} mm lead</dd>
            <dt>Spacing</dt><dd>{mat.spacing} mm</dd>
            <dt>Fibre</dt><dd>{mat.angle_tolerance > 0 ? `±${mat.angle_tolerance}°` : "exact"}</dd>
          </dl>
          {#if libDiffers}
            <p class="note">The library version of this material has changed.
              <button class="link" disabled={running} onclick={() => libEntry && pickMaterial(libEntry.name)}>Use the library version</button></p>
          {:else if !libEntry}
            <p class="note">This material is only stored in the project.
              <button class="link" onclick={() => mat && onaddmaterial(mat)}>Add it to the library</button></p>
          {/if}
        {/if}
        <button class="manage" onclick={onmanage}>Manage materials…</button>
      {/if}
    </section>

    <section>
      <h4>Time</h4>
      <div class="seg">
        {#each PRESETS as p}
          <button class:on={job.time_s === p.s} disabled={running} onclick={() => setTime(p.s)}>{p.label}</button>
        {/each}
        <label class="custom" title="Optimisation time in seconds">
          <input type="number" min="1" max="3600" value={job.time_s} disabled={running} onchange={(e) => setTime(Number((e.currentTarget as HTMLInputElement).value))} /> s
        </label>
      </div>
      <p class="hint">Longer runs usually save a little more material. You can stop at any time and keep the best layout so far.</p>
    </section>

    <section>
      <h4>Sets</h4>
      <label class="sets">
        <input type="number" min="1" max="1000" step="1" value={job.sets} disabled={running} onchange={(e) => setSets(Number((e.currentTarget as HTMLInputElement).value))} />
        complete layup{job.sets === 1 ? "" : "s"}
      </label>
      <p class="hint">Every ply quantity is multiplied by this.</p>
    </section>

    <section class="go">
      {#if !view.nest_ready.ok}
        <p class="blocked">{view.nest_ready.message}</p>
        {#if !view.ready.ok}<button class="link" onclick={onplies}>Go to the plies</button>{/if}
      {:else}
        <p class="hint">{view.nest_ready.message}</p>
      {/if}
      {#if running}
        <button class="big stop" onclick={onstop} disabled={live?.stopping}>{live?.stopping ? "Stopping…" : "Stop and keep best"}</button>
      {:else}
        <button class="big primary" onclick={onstart} disabled={!view.nest_ready.ok}>{result ? "Nest again" : "Nest"}</button>
      {/if}
      {#if live}
        <div class="progress" title="{live.phase}">
          <div class="fill" style="width:{Math.min(100, (100 * live.elapsed) / live.time_s)}%"></div>
        </div>
        <p class="hint">{live.phase === "compress" ? "Compressing" : "Exploring"} · {fmtTime(live.elapsed)} of {fmtTime(live.time_s)}</p>
      {/if}
    </section>
  </aside>

  <section class="stage">
    {#if canvas}
      <RollView bind:this={roll} shapes={canvas.shapes} placements={canvas.placements} usedLength={canvas.used} live={canvas.live} />
      <button class="fit" onclick={() => roll?.fit()} title="Fit (double-click)">Fit</button>
      {#if live && !live.placements}<div class="banner">Building a first layout…</div>{/if}
    {:else}
      <div class="empty">
        <p>Choose a material and press <b>Nest</b>. The best layout found so far is shown while the solver runs.</p>
      </div>
    {/if}
  </section>

  <aside class="result">
    {#if live}
      <h4>Nesting…</h4>
      <dl class="stats">
        <dt>Length</dt><dd class="num">{live.length != null ? `${live.length.toFixed(1)} mm` : "–"}</dd>
        <dt>Density</dt><dd class="num">{live.density != null ? pct(live.density) : "–"}</dd>
        <dt>Time</dt><dd class="num">{fmtTime(live.elapsed)}</dd>
      </dl>
      <p class="hint">Density is the solver's own measure; the final numbers are computed from the exact outlines.</p>
    {:else if result}
      {#if !result.fresh}
        <div class="stale">
          <b>Out of date.</b> {result.stale_reason}
          <button onclick={onstart} disabled={!view.nest_ready.ok}>Nest again</button>
        </div>
      {/if}
      <div class="verdict" class:ok={result.summary.verification.ok}>
        {result.summary.verification.ok ? "✓ Verification passed" : "✗ Verification failed"}
      </div>
      {#if !result.summary.verification.ok}
        <ul class="issues">{#each result.summary.verification.issues as i}<li>{i}</li>{/each}</ul>
      {/if}
      <dl class="stats">
        <dt>Used length</dt><dd class="num big-num">{result.summary.used_length_mm.toFixed(1)} mm</dd>
        <dt>Roll area</dt><dd class="num">{result.summary.roll_area_m2.toFixed(3)} m²</dd>
        <dt>Utilisation</dt><dd class="num">{pct(result.summary.utilization_of_roll)} of roll</dd>
        <dt></dt><dd class="num">{pct(result.summary.utilization_of_usable_area)} of usable</dd>
        {#if result.summary.material_cost != null}<dt>Material cost</dt><dd class="num">{result.summary.material_cost.toFixed(2)}</dd>{/if}
        <dt>Plies</dt><dd class="num">{result.summary.plies}</dd>
        <dt>Smallest gap</dt><dd class="num">{result.summary.verification.min_gap != null ? `${result.summary.verification.min_gap.toFixed(3)} mm` : "–"}</dd>
        <dt>Fibre error</dt><dd class="num">{result.summary.verification.max_fibre_deviation.toFixed(4)}°</dd>
        <dt>Material</dt><dd>{result.shapes.material.name}</dd>
        <dt>Solved in</dt><dd class="num">{fmtTime(result.run.solve_seconds)}{result.run.stopped ? " (stopped)" : ""}</dd>
      </dl>
      <button class="primary big" onclick={onexport} disabled={!!exportBlock} title={exportBlock ?? "Write the cut files, report.pdf and labels.csv"}>Export cut files…</button>
      {#if exportBlock}<p class="hint">{exportBlock}.</p>{:else}<p class="hint">Writes layout.dxf (CUT, MARK, LABEL, ROLL layers), report.pdf (numbers, layout, kit checklist), labels.csv for label printers, plies.csv, layout.svg and report.json.</p>{/if}
      {#if result.warnings.length}
        <details><summary>{result.warnings.length} warning{result.warnings.length === 1 ? "" : "s"}</summary><ul>{#each result.warnings as w}<li>{w}</li>{/each}</ul></details>
      {/if}
    {:else}
      <p class="hint">No nest yet.</p>
    {/if}
  </aside>
</div>

<style>
  .nest {
    display: grid;
    grid-template-columns: 300px 1fr 300px;
    min-height: 0;
    height: 100%;
  }
  aside {
    overflow-y: auto;
    background: var(--panel);
    padding: 4px 14px 14px;
  }
  .job {
    border-right: 1px solid var(--line);
  }
  .result {
    border-left: 1px solid var(--line);
    padding-top: 12px;
  }
  section {
    padding: 10px 0;
    border-bottom: 1px solid var(--line);
  }
  section:last-child {
    border-bottom: none;
  }
  h4 {
    margin: 0 0 8px;
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }
  select {
    width: 100%;
    font: inherit;
    padding: 5px;
    border-radius: 6px;
    border: 1px solid var(--line-strong);
    background: var(--field);
    color: var(--ink);
  }
  .facts,
  .stats {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 10px;
    margin: 10px 0;
    font-size: 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
  .num {
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .stats {
    font-size: 13px;
  }
  .big-num {
    font-weight: 700;
  }
  .note {
    font-size: 12px;
    background: var(--warn-soft);
    border-radius: 6px;
    padding: 6px 8px;
    margin: 6px 0;
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 0;
    text-decoration: underline;
    font-size: 12px;
  }
  .manage {
    width: 100%;
    margin-top: 4px;
  }
  .seg {
    display: flex;
    gap: 4px;
    align-items: center;
    flex-wrap: wrap;
  }
  .seg button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .custom {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .custom input {
    width: 64px;
  }
  .sets {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sets input {
    width: 70px;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
    margin: 6px 0 0;
    line-height: 1.4;
  }
  .blocked {
    font-size: 12px;
    white-space: pre-wrap;
    background: var(--warn-soft);
    border-radius: 6px;
    padding: 6px 8px;
    margin: 0 0 6px;
  }
  .big {
    width: 100%;
    font-size: 15px;
    padding: 9px;
    margin-top: 10px;
  }
  .stop {
    border-color: var(--bad);
    color: var(--bad);
  }
  .progress {
    height: 6px;
    border-radius: 3px;
    background: var(--line);
    overflow: hidden;
    margin-top: 10px;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.3s linear;
  }
  .stage {
    position: relative;
    min-width: 0;
    min-height: 0;
    background: var(--canvas);
    padding: 0;
    border: none;
  }
  .fit {
    position: absolute;
    top: 10px;
    right: 10px;
  }
  .banner {
    position: absolute;
    top: 12px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 7px 12px;
    pointer-events: none;
  }
  .empty {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--muted);
    text-align: center;
    padding: 40px;
  }
  .stale {
    background: var(--warn-soft);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 12px;
    margin-bottom: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
  }
  .verdict {
    font-weight: 600;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--bad-soft);
    color: var(--bad);
  }
  .verdict.ok {
    background: var(--ok-soft);
    color: var(--ok);
  }
  .issues {
    font-size: 12px;
    color: var(--bad);
    padding-left: 18px;
  }
  details {
    margin-top: 12px;
    font-size: 12px;
  }
</style>
