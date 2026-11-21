<script lang="ts">
  // The roll inventory: rolls.toml next to the material library. A roll names
  // its material by the library name; a job picks a roll by id.
  import type { LibraryView, Roll, RollsView } from "./types";

  let {
    rolls,
    library,
    material,
    current,
    onsave,
    ondelete,
    onuse,
    onclose,
  }: {
    rolls: RollsView;
    library: LibraryView;
    /** The job's material name, to preselect and to check "Use for job". */
    material: string | null;
    /** The job's roll id. */
    current: string | null;
    onsave: (original: string | null, r: Roll) => Promise<boolean>;
    ondelete: (id: string) => Promise<boolean>;
    onuse: (id: string) => void;
    onclose: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  const blank = (): Roll => ({ id: "", material: material ?? library.materials[0]?.name ?? "", batch: "", remaining_m: 50, expires: null, note: "" });

  let original = $state<string | null>(null);
  let form = $state<Roll>(blank());
  let started = false;

  function edit(r: Roll | null) {
    original = r?.id ?? null;
    form = r ? { ...structuredClone($state.snapshot(r)), expires: r.expires ?? null, note: r.note ?? "" } : blank();
  }
  $effect(() => {
    if (started) return;
    started = true;
    edit(rolls.rolls.find((r) => r.id === current) ?? rolls.rolls.find((r) => r.material === material) ?? rolls.rolls[0] ?? null);
  });

  const saved = $derived(original != null ? rolls.rolls.find((r) => r.id === original) : undefined);
  const strip = (r: Roll) => JSON.stringify([r.id, r.material, r.batch, r.remaining_m, r.expires ?? null, r.note ?? ""]);
  const changed = $derived(!saved || strip(saved) !== strip(form));
  const known = $derived(library.materials.some((m) => m.name === form.material));
  // Rolls grouped by material: the job's material first.
  const sorted = $derived(
    [...rolls.rolls].sort((a, b) => Number(b.material === material) - Number(a.material === material) || a.material.localeCompare(b.material) || a.id.localeCompare(b.id)),
  );
  const expired = (r: Roll) => !!r.expires && r.expires < rolls.today;

  let problem = $state<string | null>(null);
  async function save() {
    if (typeof form.remaining_m !== "number" || !Number.isFinite(form.remaining_m)) {
      problem = "Fill in the length left on the roll, in metres.";
      return;
    }
    problem = null;
    const r: Roll = { ...$state.snapshot(form), id: form.id.trim(), batch: form.batch.trim(), expires: form.expires || null, note: (form.note ?? "").trim() };
    if (await onsave(original, r)) original = r.id;
  }
  async function remove() {
    if (original && (await ondelete(original))) edit(rolls.rolls.find((r) => r.id !== original) ?? null);
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" {onkeydown} onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-label="Roll inventory">
    <header>
      <h3>Rolls</h3>
      <span class="path" title={rolls.path}>{rolls.path}</span>
      <button onclick={onclose}>Close</button>
    </header>
    {#if rolls.error}
      <p class="error">{rolls.error}</p>
    {/if}
    <div class="body">
      <div class="list">
        {#each sorted as r (r.id)}
          <button class:on={r.id === original} onclick={() => edit(r)}>
            <b>{r.id}{r.id === current ? " · in job" : ""}</b>
            <span class:bad={expired(r)}>{r.material} · {r.remaining_m.toFixed(2)} m{r.batch ? ` · ${r.batch}` : ""}{expired(r) ? " · expired" : ""}</span>
          </button>
        {:else}
          <p class="hint">No rolls yet. Fill in the form to add the first one.</p>
        {/each}
        <div class="list-actions">
          <button onclick={() => edit(null)}>New</button>
        </div>
      </div>

      <form class="form" onsubmit={(e) => (e.preventDefault(), save())}>
        <label>Roll id <input bind:value={form.id} placeholder="the label on the roll, e.g. R-104" /></label>
        <label
          >Material
          <select bind:value={form.material}>
            {#if form.material && !known}<option value={form.material}>{form.material} (not in the library)</option>{/if}
            {#each library.materials as m (m.name)}<option value={m.name}>{m.name}</option>{/each}
          </select>
        </label>
        <label>Batch / lot <input bind:value={form.batch} placeholder="from the supplier's label" /></label>
        <label>Length left <span class="u"><input type="number" min="0" step="any" bind:value={form.remaining_m} /> m</span></label>
        <label
          >Use by <input
            type="date"
            value={form.expires ?? ""}
            oninput={(e) => (form.expires = (e.currentTarget as HTMLInputElement).value || null)}
          /></label
        >
        <label>Note <input bind:value={form.note} placeholder="e.g. freezer 2, shelf B" /></label>
        {#if form.material && !known}<p class="wide hint bad">This material is not in the library (renamed or deleted?). Pick the right one.</p>{/if}
        {#if problem}<p class="wide hint bad">{problem}</p>{/if}
        <div class="wide actions">
          {#if original}<button type="button" class="danger" onclick={remove}>Delete</button>{/if}
          <span class="spacer"></span>
          <button type="submit" disabled={!changed || !form.id.trim() || !form.material}>{original ? "Save changes" : "Add roll"}</button>
          <button
            type="button"
            class="primary"
            disabled={changed || !original || !saved || saved.material !== material}
            onclick={() => saved && onuse(saved.id)}
            title={changed ? "Save first" : saved && saved.material !== material ? `The job is on ${material ?? "no material"}` : "Cut this job from this roll"}>Use for job</button
          >
        </div>

        {#if saved?.usage?.length}
          <div class="wide history">
            <h4>Booked on this roll</h4>
            <table>
              <thead><tr><th>Date</th><th>Job</th><th class="n">Length</th></tr></thead>
              <tbody>
                {#each [...saved.usage].reverse() as u (u.result)}
                  <tr><td>{u.date}</td><td>{u.job}</td><td class="n">{u.length_m.toFixed(2)} m</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
        <p class="wide hint">
          Book a nest on its roll from the Nest view once it is cut: the used length comes off the roll. Edit the length here after measuring a roll.
        </p>
      </form>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.3);
    display: grid;
    place-items: center;
    z-index: 40;
  }
  .dialog {
    width: min(820px, calc(100vw - 40px));
    max-height: calc(100vh - 60px);
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 12px;
    box-shadow: 0 12px 40px rgb(0 0 0 / 0.25);
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--line);
  }
  h3 {
    margin: 0;
  }
  h4 {
    margin: 0 0 6px;
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .path {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .error {
    margin: 10px 16px 0;
    padding: 8px 10px;
    background: var(--bad-soft);
    color: var(--bad);
    border-radius: 6px;
    white-space: pre-wrap;
  }
  .body {
    display: grid;
    grid-template-columns: 240px 1fr;
    min-height: 0;
    overflow: hidden;
  }
  .list {
    border-right: 1px solid var(--line);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
  }
  .list > button {
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 2px;
    border-color: transparent;
    background: transparent;
  }
  .list > button span {
    font-size: 11px;
    color: var(--muted);
  }
  .list > button span.bad {
    color: var(--bad);
  }
  .list > button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .list-actions {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 14px;
    padding: 14px 16px;
    overflow-y: auto;
    align-content: start;
  }
  .form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }
  .form select {
    font: inherit;
    padding: 4px;
    border-radius: 6px;
    border: 1px solid var(--line-strong);
    background: var(--field);
    color: var(--ink);
  }
  .wide {
    grid-column: 1 / -1;
  }
  .u {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .u input {
    width: 90px;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
    margin: 0;
  }
  .bad {
    color: var(--bad);
  }
  .actions {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 4px;
  }
  .spacer {
    flex: 1;
  }
  .danger {
    color: var(--bad);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  th {
    text-align: left;
    font-weight: 600;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
    padding: 3px 6px;
  }
  td {
    padding: 3px 6px;
    border-bottom: 1px solid var(--line);
  }
  .n {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
</style>
