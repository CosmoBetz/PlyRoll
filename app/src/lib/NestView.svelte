<script lang="ts">
  import type { AppView, Edit, Grouping, JobSettings, LibraryView, LiveNest as Live, Material, Paper, Roll, RollsView } from "./types";
  import RollView from "./RollView.svelte";

  let {
    view,
    library,
    rolls,
    live,
    onjob,
    onstart,
    onstop,
    onexport,
    onmanage,
    onaddmaterial,
    onplies,
    onmanagerolls,
    onbook,
    onunbook,
    onedit,
    onundoedit,
    keys = true,
  }: {
    view: AppView;
    library: LibraryView;
    rolls: RollsView;
    live: Live | null;
    onjob: (job: JobSettings) => void;
    /** With `keepLocked`, locked plies stay and the rest is nested after them. */
    onstart: (keepLocked?: boolean) => void;
    onstop: () => void;
    onexport: () => void;
    onmanage: () => void;
    onaddmaterial: (m: Material) => void;
    onplies: () => void;
    onmanagerolls: () => void;
    onbook: () => void;
    onunbook: () => void;
    /** A hand edit of the result; false when it was refused. */
    onedit: (e: Edit) => Promise<boolean>;
    onundoedit: (redo: boolean) => void;
    /** Keyboard shortcuts are active (no dialog open). */
    keys?: boolean;
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
    // A roll of another material no longer fits the job.
    if (m) onjob({ ...job, material: structuredClone($state.snapshot(m)), roll: jobRoll && jobRoll.material !== name ? null : (job.roll ?? null) });
  }

  // ---- roll inventory
  const jobRoll = $derived(job.roll ? (rolls.rolls.find((r) => r.id === job.roll) ?? null) : null);
  const rollChoices = $derived(mat ? rolls.rolls.filter((r) => r.material === mat.name) : []);
  const expired = (r: Roll) => !!r.expires && r.expires < rolls.today;
  // Where the current nest is booked, if anywhere.
  const booking = $derived.by(() => {
    if (!result) return null;
    for (const r of rolls.rolls) {
      const u = r.usage?.find((x) => x.result === result.key);
      if (u) return { roll: r, usage: u };
    }
    return null;
  });
  // Length the job's roll has for this nest (a booking of this nest counted back).
  const rollLeft = $derived(jobRoll ? jobRoll.remaining_m + (booking && booking.roll.id === jobRoll.id ? booking.usage.length_m : 0) : null);
  const need = $derived(live ? (live.length != null && live.placements ? live.length / 1000 : null) : result ? result.used_length / 1000 : null);
  const short = $derived(rollLeft != null && need != null && need > rollLeft + 1e-9);
  function pickRoll(id: string) {
    if ((id || null) !== (job.roll ?? null)) onjob({ ...job, roll: id || null });
  }

  // ---- 1:1 templates
  const PAPERS: { value: string; label: string }[] = [
    { value: "", label: "No templates" },
    { value: "a4", label: "A4 sheets" },
    { value: "a3", label: "A3 sheets" },
    { value: "letter", label: "Letter sheets" },
    { value: "roll", label: "Plotter roll" },
  ];
  const paper = $derived(job.templates ?? null);
  function setPaper(kind: string) {
    const t: Paper | null =
      kind === "" ? null : kind === "roll" ? { kind: "roll", width_mm: paper?.kind === "roll" ? paper.width_mm : 914 } : ({ kind } as Paper);
    onjob({ ...job, templates: t });
  }
  function setPlotterWidth(w: number) {
    if (Number.isFinite(w) && w >= 100 && w <= 5000 && !(paper?.kind === "roll" && paper.width_mm === w)) onjob({ ...job, templates: { kind: "roll", width_mm: w } });
  }
  const paperName = (p: Paper) => (p.kind === "roll" ? `a ${p.width_mm} mm plotter roll` : p.kind === "letter" ? "Letter" : p.kind.toUpperCase());
  function setTime(s: number) {
    if (Number.isFinite(s) && s >= 1 && s <= 3600 && s !== job.time_s) onjob({ ...job, time_s: Math.round(s) });
  }
  function setGrouping(g: Grouping) {
    if (g !== job.grouping) onjob({ ...job, grouping: g });
  }
  // Kits with plies to cut, in project order (no kit last), as nested kit by kit.
  const kits = $derived.by(() => {
    const seen: (string | null)[] = [];
    for (const v of view.plies.flatMap((p) => p.variants)) if (v.qty > 0 && !seen.includes(v.kit ?? null)) seen.push(v.kit ?? null);
    return [...seen.filter((k) => k != null), ...seen.filter((k) => k == null)];
  });
  const kitName = (k: string | null) => k ?? "no kit";
  function setSets(n: number) {
    if (Number.isInteger(n) && n >= 1 && n <= 1000 && n !== job.sets) onjob({ ...job, sets: n });
  }
  const fmtTime = (s: number) => (s >= 60 ? `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")} min` : `${s.toFixed(0)} s`);
  const pct = (x: number) => `${(100 * x).toFixed(1)}%`;

  // What the canvas shows: the running nest, else the stored result.
  const canvas = $derived.by(() => {
    if (live) return { shapes: live.shapes, placements: live.placements ?? [], used: live.placements ? live.length : null, live: true, sections: live.sections };
    if (result) return { shapes: result.shapes, placements: result.placements, used: result.used_length, live: false, sections: result.sections ?? [] };
    return null;
  });
  // ---- editing by hand
  let selected = $state<number[]>([]);
  // A new nest starts with nothing selected; edits keep the selection.
  let selFor = "";
  $effect(() => {
    const k = result ? `${result.key}|${result.run.solve_seconds}|${result.placements.length}` : "";
    if (k !== selFor) {
      selFor = k;
      selected = [];
    }
  });
  const editBlock = $derived(
    !result || live || view.running != null
      ? "Nesting…"
      : !result.fresh
        ? "Out of date: nest again first."
        : booking
          ? `Booked on roll ${booking.roll.id}: take the booking back to change the layout.`
          : null,
  );
  const canEdit = $derived(!!result && !editBlock);
  // Nesting again can keep the locked plies (mixed layouts, current result).
  const keepable = $derived(!!result && result.fresh && result.locked.length > 0 && result.sections.length === 0 && result.locked.length < result.placements.length);
  const lockedSet = $derived(new Set(result?.locked ?? []));
  const allLocked = $derived(selected.length > 0 && selected.every((i) => lockedSet.has(i)));
  const anyLocked = $derived(selected.some((i) => lockedSet.has(i)));
  const one = $derived(selected.length === 1 && result ? result.placements[selected[0]] : null);
  const fineStep = $derived(result && result.shapes.material.angle_tolerance > 0 ? result.shapes.material.tolerance_step : 0);
  // Edits go out one after another, so a held arrow key never shows an older layout last.
  let queue: Promise<unknown> = Promise.resolve();
  function send(e: Edit) {
    if (!canEdit) return;
    queue = queue.then(() => onedit(e));
  }
  const move = (dx: number, dy: number) => selected.length && send({ op: "move", indices: [...selected], dx, dy });
  const turn = (sign: number) => one && send({ op: "rotate", index: selected[0], by: sign * one.turn });
  const fine = (sign: number) => one && fineStep && send({ op: "rotate", index: selected[0], by: sign * fineStep });
  const toggleLock = () => selected.length && send({ op: "lock", indices: [...selected], on: !allLocked });
  function onkeydown(e: KeyboardEvent) {
    if (!keys || !canEdit || e.metaKey || e.ctrlKey) return;
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT") return;
    const step = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
    const arrows: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] };
    const k = e.key.toLowerCase();
    if (arrows[e.key] && selected.length) move(...arrows[e.key]);
    else if (k === "r") turn(e.shiftKey ? -1 : 1);
    else if (e.key === "[" || e.code === "BracketLeft") fine(-1);
    else if (e.key === "]" || e.code === "BracketRight") fine(1);
    else if (k === "l") toggleLock();
    else if (e.key === "Escape" && selected.length) selected = [];
    else return;
    e.preventDefault();
  }

  const exportBlock = $derived(
    !result ? "Nothing nested yet" : !result.fresh ? "Out of date: nest again first" : !result.summary.verification.ok ? "Failed verification" : null,
  );
</script>

<svelte:window {onkeydown} />

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
      <h4>Roll</h4>
      {#if !mat}
        <p class="hint">Choose a material first.</p>
      {:else}
        <select value={job.roll ?? ""} onchange={(e) => pickRoll((e.currentTarget as HTMLSelectElement).value)}>
          <option value="">No roll picked</option>
          {#if job.roll && (!jobRoll || jobRoll.material !== mat.name)}<option value={job.roll}>{job.roll}</option>{/if}
          {#each rollChoices as r (r.id)}
            <option value={r.id}>{r.id} · {r.remaining_m.toFixed(2)} m{r.batch ? ` · ${r.batch}` : ""}{expired(r) ? " · expired" : ""}</option>
          {/each}
        </select>
        {#if job.roll && !jobRoll}
          <p class="note">Roll {job.roll} is not in the inventory. Pick another roll, or none.</p>
        {:else if jobRoll && jobRoll.material !== mat.name}
          <p class="note">Roll {jobRoll.id} is {jobRoll.material}, not {mat.name}.</p>
        {:else if jobRoll}
          <dl class="facts">
            {#if jobRoll.batch}<dt>Batch</dt><dd>{jobRoll.batch}</dd>{/if}
            <dt>Left</dt><dd>{jobRoll.remaining_m.toFixed(2)} m</dd>
            {#if jobRoll.expires}<dt>Use by</dt><dd class:bad={expired(jobRoll)}>{jobRoll.expires}{expired(jobRoll) ? " (expired)" : ""}</dd>{/if}
          </dl>
          {#if expired(jobRoll)}<p class="note">This roll is past its use-by date.</p>{/if}
          {#if short && need != null && rollLeft != null}
            <p class="note bad-note">Not enough roll: {live ? "the nest so far needs" : "this nest needs"} {need.toFixed(2)} m, roll {jobRoll.id} has {rollLeft.toFixed(2)} m.</p>
          {/if}
        {:else if rollChoices.length === 0}
          <p class="hint">No {mat.name} rolls in the inventory.</p>
        {:else}
          <p class="hint">Optional: the roll's batch goes on the report and labels, and you get a warning if the nest needs more than it has.</p>
        {/if}
        <button class="manage" onclick={onmanagerolls}>Manage rolls…</button>
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

    <section>
      <h4>Layout</h4>
      <div class="seg">
        <button class:on={job.grouping === "global"} disabled={running} onclick={() => setGrouping("global")} title="All plies mixed: the shortest roll">Mixed</button>
        <button class:on={job.grouping === "by-kit"} disabled={running} onclick={() => setGrouping("by-kit")} title="Each kit on its own stretch of roll, cut together"
          >Kit by kit</button
        >
      </div>
      <p class="hint">
        {#if job.grouping === "by-kit"}
          {#if kits.length > 1}
            Each kit gets its own stretch of roll, in this order: {kits.map(kitName).join(", ")}. Usually a little more material than mixed.
          {:else}
            Only one kit ({kitName(kits[0] ?? null)}), so this is the same as mixed. Set kits in the ply angle table.
          {/if}
        {:else}
          All plies mixed for the shortest roll. Kit by kit keeps each kit's plies together.
        {/if}
      </p>
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
        {#if keepable}
          <button class="big primary" onclick={() => onstart(true)} disabled={!view.nest_ready.ok}>Nest the unlocked plies</button>
          <p class="hint">
            Keeps the {result!.locked.length} locked pl{result!.locked.length === 1 ? "y" : "ies"} where {result!.locked.length === 1 ? "it is" : "they are"} and nests
            the rest on the roll after the last of them. Gaps between locked plies are not filled.
          </p>
          <button class="big" onclick={() => onstart(false)} disabled={!view.nest_ready.ok}>Nest everything again</button>
        {:else}
          <button class="big primary" onclick={() => onstart(false)} disabled={!view.nest_ready.ok}>{result ? "Nest again" : "Nest"}</button>
          {#if result?.fresh && result.locked.length && result.sections.length}
            <p class="hint">Kit by kit, locked plies cannot be kept when nesting again: nesting replaces the whole layout. Keeping locked plies works on nests made with the Mixed layout.</p>
          {/if}
        {/if}
      {/if}
      {#if live}
        <div class="progress" title="{live.phase}">
          <div class="fill" style="width:{Math.min(100, (100 * live.elapsed) / live.time_s)}%"></div>
        </div>
        <p class="hint">
          {#if live.section}Kit {kitName(live.section.kit)} ({live.section.index + 1} of {live.section.count}) ·
          {/if}{live.phase === "compress" ? "Compressing" : "Exploring"} · {fmtTime(live.elapsed)} of {fmtTime(live.time_s)}
        </p>
      {/if}
    </section>
  </aside>

  <section class="stage">
    {#if canvas}
      <RollView
        bind:this={roll}
        shapes={canvas.shapes}
        placements={canvas.placements}
        usedLength={canvas.used}
        live={canvas.live}
        sections={canvas.sections}
        editable={!canvas.live && canEdit}
        {selected}
        locked={canvas.live ? [] : (result?.locked ?? [])}
        bad={canvas.live || !result ? [] : result.summary.verification.bad}
        frameKey={result?.key ?? ""}
        onselect={(idx) => (selected = idx)}
        onmove={(idx, dx, dy) => send({ op: "move", indices: idx, dx, dy })}
      />
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
          <button onclick={() => onstart(false)} disabled={!view.nest_ready.ok}>Nest again</button>
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
        {#if result.run.kept}<dt>Kept locked</dt><dd class="num">{result.run.kept}</dd>{/if}
        {#if result.run.edits}<dt>Hand edits</dt><dd class="num">{result.run.edits}</dd>{/if}
      </dl>
      <div class="editbox">
        <h4>Edit by hand</h4>
        {#if editBlock}
          <p class="hint">{editBlock}</p>
        {:else}
          {#if one}
            <p class="sel-line"><b>{one.label}</b> copy {one.copy}{lockedSet.has(selected[0]) ? " · locked" : ""}</p>
          {:else if selected.length}
            <p class="sel-line"><b>{selected.length} plies</b> selected{anyLocked ? (allLocked ? " · locked" : " · some locked") : ""}</p>
          {:else}
            <p class="hint">Click a ply to select it (⇧-click adds more), then drag it or use the keys below.</p>
          {/if}
          <div class="tools">
            <button onclick={() => turn(1)} disabled={!one || anyLocked} title={one ? `Turn ${one.turn}° about its centre (R; ⇧R the other way)` : "Select one ply"}
              >Turn {one?.turn ?? 180}°</button
            >
            {#if fineStep}
              <button onclick={() => fine(-1)} disabled={!one || anyLocked} title="Turn clockwise within the fibre tolerance ([)">−{fineStep}°</button>
              <button onclick={() => fine(1)} disabled={!one || anyLocked} title="Turn counter-clockwise within the fibre tolerance (])">+{fineStep}°</button>
            {/if}
            <button onclick={toggleLock} disabled={!selected.length} title="Locked plies cannot be moved or turned (L)">{allLocked ? "Unlock" : "Lock"}</button>
          </div>
          <div class="tools">
            <button onclick={() => onundoedit(false)} disabled={!view.can_undo_edit} title="Undo the last move, turn or lock (⌘Z)">Undo</button>
            <button onclick={() => onundoedit(true)} disabled={!view.can_redo_edit} title="Redo (⇧⌘Z)">Redo</button>
            {#if result.summary.verification.bad.length}
              <button class="link" onclick={() => (selected = [...result.summary.verification.bad])}>Select the plies with problems</button>
            {/if}
          </div>
          <p class="hint">
            Arrows nudge 1 mm (⇧ 10 mm, ⌥ 0.1 mm) · R turns · {fineStep ? "[ ] fine turn · " : ""}L locks · Esc clears. Turns keep the fibres along the roll. Every
            change is checked again; export needs a pass.
          </p>
        {/if}
      </div>
      <div class="tpl">
        <label for="paper">1:1 templates</label>
        <select id="paper" value={paper?.kind ?? ""} onchange={(e) => setPaper((e.currentTarget as HTMLSelectElement).value)}>
          {#each PAPERS as p (p.value)}<option value={p.value}>{p.label}</option>{/each}
        </select>
        {#if paper?.kind === "roll"}
          <span class="u"
            ><input type="number" min="100" max="5000" step="1" value={paper.width_mm} onchange={(e) => setPlotterWidth(Number((e.currentTarget as HTMLInputElement).value))} /> mm wide</span
          >
        {/if}
      </div>
      <button class="primary big" onclick={onexport} disabled={!!exportBlock} title={exportBlock ?? "Write the cut files, report.pdf and labels.csv"}>Export cut files…</button>
      {#if exportBlock}<p class="hint">{exportBlock}.</p>{:else}<p class="hint">
          Writes layout.dxf (CUT, MARK, LABEL, ROLL layers), report.pdf (numbers, layout, kit checklist), labels.csv for label printers, plies.csv, layout.svg and
          report.json{#if paper}, and templates.pdf: the layout at true size on {paperName(paper)}, to print at 100% and tape together{/if}.
        </p>{/if}
      {#if booking}
        <div class="booking">
          Booked {booking.usage.length_m.toFixed(2)} m on roll <b>{booking.roll.id}</b> ({booking.usage.date}).
          <button class="link" onclick={onunbook}>Take back</button>
        </div>
      {:else if !exportBlock && jobRoll && mat && jobRoll.material === mat.name}
        <button class="big" onclick={onbook} disabled={short} title={short ? "The roll is too short for this nest" : "Once the job is cut"}
          >Book {(result.used_length / 1000).toFixed(2)} m on roll {jobRoll.id}</button
        >
        <p class="hint">Once the job is cut: takes the used length off the roll in the inventory. Can be taken back.</p>
      {/if}
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
  .bad-note {
    background: var(--bad-soft);
    color: var(--bad);
  }
  dd.bad {
    color: var(--bad);
  }
  .tpl {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 14px;
    font-size: 12px;
    color: var(--muted);
  }
  .tpl select {
    width: auto;
    flex: 1;
  }
  .tpl .u {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .tpl input {
    width: 70px;
  }
  .editbox {
    border-top: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
    padding: 10px 0;
    margin-bottom: 12px;
  }
  .editbox .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    margin: 6px 0;
  }
  .sel-line {
    font-size: 12px;
    margin: 0 0 6px;
  }
  .booking {
    margin-top: 12px;
    font-size: 12px;
    background: var(--ok-soft);
    border-radius: 6px;
    padding: 6px 8px;
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
