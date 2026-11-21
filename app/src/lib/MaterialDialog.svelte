<script lang="ts">
  // The material library: one TOML file in the app's config folder, the same
  // format the CLI reads with --materials. Editing it never changes a project;
  // a project keeps its own copy until you pick the material again.
  import type { LibraryView, Material } from "./types";

  let {
    library,
    current,
    onsave,
    ondelete,
    onimport,
    onuse,
    onclose,
  }: {
    library: LibraryView;
    /** Name of the project's material, to preselect it. */
    current: string | null;
    onsave: (original: string | null, m: Material) => Promise<boolean>;
    ondelete: (name: string) => Promise<boolean>;
    onimport: () => void;
    onuse: (m: Material) => void;
    onclose: () => void;
  } = $props();

  const blank = (): Material => ({
    name: "",
    kind: "ud",
    roll_width: 1000,
    edge_trim: 10,
    lead_trim: 20,
    spacing: 2,
    angle_tolerance: 0,
    tolerance_step: 0.5,
    warp_weft_interchangeable: false,
    cost_per_m2: null,
  });

  // `original` = name of the library entry being edited; null = a new one.
  let original = $state<string | null>(null);
  let form = $state<Material>(blank());
  let started = false;

  function edit(m: Material | null) {
    original = m?.name ?? null;
    form = m ? structuredClone($state.snapshot(m)) : blank();
  }
  $effect(() => {
    if (started) return;
    started = true;
    edit(library.materials.find((m) => m.name === current) ?? library.materials[0] ?? null);
  });

  const saved = $derived(original != null ? library.materials.find((m) => m.name === original) : undefined);
  const changed = $derived(!saved || JSON.stringify(saved) !== JSON.stringify(form));
  const usable = $derived(form.roll_width - 2 * form.edge_trim);

  let problem = $state<string | null>(null);
  async function save() {
    const nums = [form.roll_width, form.edge_trim, form.lead_trim, form.spacing, form.angle_tolerance, form.tolerance_step];
    if (nums.some((v) => typeof v !== "number" || !Number.isFinite(v))) {
      problem = "Fill in every number (lengths in mm).";
      return;
    }
    problem = null;
    const m: Material = { ...form, name: form.name.trim(), warp_weft_interchangeable: form.kind === "twill" && form.warp_weft_interchangeable };
    if (await onsave(original, m)) original = m.name;
  }
  async function remove() {
    if (original && (await ondelete(original))) edit(library.materials.find((m) => m.name !== original) ?? null);
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onkeydown={onkeydown} onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog" role="dialog" aria-label="Material library">
    <header>
      <h3>Materials</h3>
      <span class="path" title={library.path}>{library.path}</span>
      <button onclick={onclose}>Close</button>
    </header>
    {#if library.error}
      <p class="error">{library.error}</p>
    {/if}
    <div class="body">
      <div class="list">
        {#each library.materials as m (m.name)}
          <button class:on={m.name === original} onclick={() => edit(m)}>
            <b>{m.name}</b>
            <span>{m.kind === "ud" ? "UD" : "Twill"} · {m.roll_width} mm{m.name === current ? " · in project" : ""}</span>
          </button>
        {:else}
          <p class="hint">No materials yet. Fill in the form to add your first one, or import a materials.toml.</p>
        {/each}
        <div class="list-actions">
          <button onclick={() => edit(null)}>New</button>
          <button onclick={onimport} title="Merge a materials.toml (CLI format); same names are replaced">Import…</button>
        </div>
      </div>

      <form class="form" onsubmit={(e) => (e.preventDefault(), save())}>
        <label class="wide">Name <input bind:value={form.name} placeholder="e.g. T700 UD 150 g/m²" /></label>
        <fieldset class="wide kind">
          <legend>Type</legend>
          <label><input type="radio" bind:group={form.kind} value="ud" /> UD</label>
          <label><input type="radio" bind:group={form.kind} value="twill" /> Twill</label>
        </fieldset>
        <label>Roll width <span class="u"><input type="number" min="1" step="any" bind:value={form.roll_width} /> mm</span></label>
        <label>Edge trim <span class="u"><input type="number" min="0" step="any" bind:value={form.edge_trim} /> mm each side</span></label>
        <label>Lead trim <span class="u"><input type="number" min="0" step="any" bind:value={form.lead_trim} /> mm at the start</span></label>
        <label>Spacing <span class="u"><input type="number" min="0" step="any" bind:value={form.spacing} /> mm between plies</span></label>
        <label
          >Fibre tolerance <span class="u"><input type="number" min="0" max="10" step="any" bind:value={form.angle_tolerance} /> ° (0 = exact)</span></label
        >
        {#if form.angle_tolerance > 0}
          <label>Tolerance step <span class="u"><input type="number" min="0.05" step="any" bind:value={form.tolerance_step} /> °</span></label>
        {/if}
        <label
          >Cost <span class="u"
            ><input
              type="number"
              min="0"
              step="any"
              value={form.cost_per_m2 ?? ""}
              oninput={(e) => {
                const v = (e.currentTarget as HTMLInputElement).value;
                form.cost_per_m2 = v === "" ? null : Number(v);
              }}
            /> per m² (optional)</span
          ></label
        >
        {#if form.kind === "twill"}
          <label class="wide check">
            <input type="checkbox" bind:checked={form.warp_weft_interchangeable} />
            Warp and weft may be swapped (allows 90° turns, except on cosmetic plies)
          </label>
        {/if}
        <p class="wide hint">Usable width: <b class:bad={usable <= 0}>{usable > 0 ? `${usable} mm` : "none"}</b> (roll minus both edge trims). Plies are never flipped.</p>
        {#if problem}<p class="wide hint bad">{problem}</p>{/if}
        <div class="wide actions">
          {#if original}<button type="button" class="danger" onclick={remove}>Delete</button>{/if}
          <span class="spacer"></span>
          <button type="submit" disabled={!changed || !form.name.trim()}>{original ? "Save changes" : "Add material"}</button>
          <button type="button" class="primary" disabled={changed || !original} onclick={() => saved && onuse(saved)} title={changed ? "Save first" : "Use this material for the project"}
            >Use for project</button
          >
        </div>
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
  .form .wide {
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
  .wide input:not([type="checkbox"]):not([type="radio"]) {
    width: 100%;
  }
  .kind {
    border: none;
    padding: 0;
    margin: 0;
    display: flex;
    gap: 16px;
    font-size: 12px;
  }
  .kind legend {
    color: var(--muted);
    margin-bottom: 4px;
    padding: 0;
  }
  .kind label,
  .form .check {
    flex-direction: row;
    align-items: center;
    color: var(--ink);
    font-size: 13px;
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
</style>
