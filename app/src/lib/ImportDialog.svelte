<script lang="ts">
  import type { ImportSettings, SourceFile, Units } from "./types";

  let {
    settings,
    sources,
    plyCount,
    onapply,
    onclose,
  }: {
    settings: ImportSettings;
    /** null while the files are being read. */
    sources: SourceFile[] | null;
    plyCount: number;
    /** Resolves true when the settings were applied. */
    onapply: (s: ImportSettings) => Promise<boolean>;
    onclose: () => void;
  } = $props();

  const UNITS: { value: Units; label: string }[] = [
    { value: "auto", label: "From each file ($INSUNITS; mm when not set)" },
    { value: "mm", label: "Every file is in mm" },
    { value: "cm", label: "Every file is in cm" },
    { value: "m", label: "Every file is in m" },
    { value: "inch", label: "Every file is in inches" },
  ];
  const MAX_TOL = 5;

  // svelte-ignore state_referenced_locally
  let form = $state<ImportSettings>({ ...settings });
  let busy = $state(false);

  const changed = $derived(form.units !== settings.units || form.join_tol !== settings.join_tol || form.gap_tol !== settings.gap_tol);
  const problem = $derived.by(() => {
    const ok = (v: number) => typeof v === "number" && Number.isFinite(v) && v > 0 && v <= MAX_TOL;
    if (!ok(form.join_tol)) return `Join tolerance: more than 0, at most ${MAX_TOL} mm.`;
    if (!ok(form.gap_tol) || form.gap_tol < form.join_tol) return `Gap tolerance: at least the join tolerance, at most ${MAX_TOL} mm.`;
    return null;
  });
  const missing = $derived(sources?.filter((s) => !s.found) ?? []);
  const unitless = $derived(sources?.filter((s) => s.file_units === "none") ?? []);

  const base = (p: string) => p.split(/[\\/]/).pop() ?? p;
  function unitsShown(s: SourceFile): string {
    if (!s.found) return "file not found";
    if (s.file_units == null) return "unreadable";
    const declared = s.file_units === "none" ? "none" : s.file_units;
    const used = form.units !== "auto" ? form.units : s.file_units === "none" ? "mm" : s.file_units;
    return declared === used ? declared : `${declared} → read as ${used}`;
  }

  async function apply() {
    if (!changed || problem || busy) return;
    busy = true;
    try {
      if (await onapply($state.snapshot(form) as ImportSettings)) onclose();
    } finally {
      busy = false;
    }
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
  <div class="dialog" role="dialog" aria-label="Import settings">
    <header>
      <h3>Import settings</h3>
      <span class="spacer"></span>
      <button onclick={onclose}>Close</button>
    </header>

    <form class="form" onsubmit={(e) => (e.preventDefault(), apply())}>
      <label class="wide"
        >Units
        <select bind:value={form.units}>
          {#each UNITS as u (u.value)}<option value={u.value}>{u.label}</option>{/each}
        </select>
      </label>
      <label>Join tolerance <span class="u"><input type="number" min="0" max={MAX_TOL} step="any" bind:value={form.join_tol} /> mm</span></label>
      <label>Gap tolerance <span class="u"><input type="number" min="0" max={MAX_TOL} step="any" bind:value={form.gap_tol} /> mm, bridged with a warning</span></label>
      <p class="wide hint">
        Endpoints closer than the join tolerance count as connected; larger gaps up to the gap tolerance are closed with a straight line. Both are in mm after
        scaling, so switching to a larger unit can turn tiny gaps into ones that need a larger gap tolerance.
      </p>

      {#if sources == null}
        <p class="wide hint">Reading the DXFs…</p>
      {:else if sources.length}
        <table class="wide">
          <thead><tr><th>DXF</th><th>Units</th><th class="n">Plies</th></tr></thead>
          <tbody>
            {#each sources as s (s.path)}
              <tr class:bad={!s.found} class:warn={s.found && s.file_units === "none" && form.units === "auto"}>
                <td title={s.path}>{base(s.path)}</td>
                <td>{unitsShown(s)}</td>
                <td class="n">{s.plies}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if unitless.length && form.units === "auto"}
          <p class="wide hint warn">
            {unitless.length === 1 ? "One file has" : `${unitless.length} files have`} no units and {unitless.length === 1 ? "is" : "are"} read as mm. If the plies look
            25× too small, the drawing is in inches: choose that above.
          </p>
        {/if}
      {/if}

      <p class="wide hint">
        {#if plyCount === 0}
          Used for the next import.
        {:else if missing.length}
          <span class="bad">{missing.length === 1 ? "A DXF is" : `${missing.length} DXFs are`} missing. Every file must be found to apply new settings.</span>
        {:else}
          Applying re-reads every DXF. 0° references are kept; a ply whose outline only changes scale stays confirmed, any other change is flagged for a
          check. Undo goes back.
        {/if}
      </p>
      {#if problem}<p class="wide hint bad">{problem}</p>{/if}
      <div class="wide actions">
        <span class="spacer"></span>
        <button type="button" onclick={onclose}>Cancel</button>
        <button type="submit" class="primary" disabled={!changed || !!problem || busy || (plyCount > 0 && missing.length > 0)}
          >{busy ? "Applying…" : plyCount ? "Apply and re-read DXFs" : "Apply"}</button
        >
      </div>
    </form>
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
    width: min(620px, calc(100vw - 40px));
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
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 16px;
    padding: 14px 16px 16px;
    overflow-y: auto;
  }
  .form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
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
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .bad {
    color: var(--bad);
  }
  .warn {
    color: var(--warn, #b26b00);
  }
  table {
    border-collapse: collapse;
    font-size: 12px;
  }
  th {
    text-align: left;
    font-weight: 600;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
    padding: 4px 6px;
  }
  td {
    padding: 4px 6px;
    border-bottom: 1px solid var(--line);
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .n {
    text-align: right;
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
</style>
