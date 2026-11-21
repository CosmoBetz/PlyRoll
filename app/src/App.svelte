<script lang="ts">
  import { onMount } from "svelte";
  import { createBackend, type Backend } from "./lib/backend";
  import type { AppView, ImportReport, Ply, RefPick, Variant } from "./lib/types";
  import { angleColor, pickLabel, variantLabel } from "./lib/geo";
  import Editor from "./lib/Editor.svelte";
  import Thumb from "./lib/Thumb.svelte";
  import Inspector from "./lib/Inspector.svelte";

  let backend = $state<Backend>();
  let view = $state<AppView>();
  let selection = $state<number[]>([]);
  let activeId = $state<number | null>(null);
  let pointsMode = $state(false);
  let preview = $state(0);
  let filter = $state<"all" | "todo">("all");
  let autoAdvance = $state(false);
  let showHelp = $state(false);
  let toasts = $state<{ id: number; kind: "info" | "error"; text: string }[]>([]);
  let editor = $state<Editor>();
  let toastSeq = 0;

  const plies = $derived(view?.plies ?? []);
  const todo = $derived(plies.filter((p) => p.status !== "ok"));
  const listed = $derived(filter === "todo" ? plies.filter((p) => p.status !== "ok" || selection.includes(p.id)) : plies);
  const active = $derived(plies.find((p) => p.id === activeId) ?? null);
  const selectedPlies = $derived(plies.filter((p) => selection.includes(p.id)));
  const targetIds = $derived(selection.length ? selection : activeId != null ? [activeId] : []);

  function toast(text: string, kind: "info" | "error" = "info") {
    const id = ++toastSeq;
    toasts = [...toasts, { id, kind, text }];
    setTimeout(() => (toasts = toasts.filter((t) => t.id !== id)), kind === "error" ? 7000 : 3500);
  }

  /** Applies a new view and keeps the selection valid. */
  function show(v: AppView) {
    view = v;
    const ids = new Set(v.plies.map((p) => p.id));
    selection = selection.filter((id) => ids.has(id));
    if (activeId == null || !ids.has(activeId)) {
      activeId = selection[0] ?? v.plies[0]?.id ?? null;
      if (activeId != null && selection.length === 0) selection = [activeId];
    }
    if (active && preview >= active.variants.length) preview = active.variants.length ? 0 : -1;
  }

  async function run(f: () => Promise<AppView>, success?: string) {
    try {
      show(await f());
      if (success) toast(success);
      return true;
    } catch (e) {
      toast(String(e), "error");
      return false;
    }
  }

  function select(id: number, how: "replace" | "toggle" | "range" = "replace") {
    if (how === "toggle") {
      selection = selection.includes(id) ? selection.filter((x) => x !== id) : [...selection, id];
      if (selection.length === 0) selection = [id];
    } else if (how === "range" && activeId != null) {
      const ids = listed.map((p) => p.id);
      const [a, b] = [ids.indexOf(activeId), ids.indexOf(id)].sort((x, y) => x - y);
      if (a >= 0 && b >= 0) selection = ids.slice(a, b + 1);
    } else selection = [id];
    activeId = id;
    pointsMode = false;
  }

  function step(dir: 1 | -1, extend = false) {
    if (listed.length === 0) return;
    const i = listed.findIndex((p) => p.id === activeId);
    const next = listed[Math.max(0, Math.min(listed.length - 1, i < 0 ? 0 : i + dir))];
    if (extend) {
      if (!selection.includes(next.id)) selection = [...selection, next.id];
      activeId = next.id;
    } else select(next.id);
  }

  function nextTodo() {
    if (todo.length === 0) return toast("Every ply has a confirmed 0° reference.");
    const order = plies.map((p) => p.id);
    const cur = activeId != null ? order.indexOf(activeId) : -1;
    const after = todo.find((p) => order.indexOf(p.id) > cur) ?? todo[0];
    select(after.id);
  }

  async function pick(pick: RefPick) {
    const ids = pick.kind === "edge" || pick.kind === "points" ? (activeId != null ? [activeId] : []) : targetIds;
    if (!backend || ids.length === 0) return;
    const wasTodo = ids.length === 1 && active?.status !== "ok";
    const ok = await run(() => backend!.setReference(ids, pick), ids.length > 1 ? `${pickLabel(pick)} on ${ids.length} plies` : undefined);
    if (ok && autoAdvance && wasTodo && pick.kind !== "rotate" && todo.length > 0) nextTodo();
  }

  const setVariants = (vs: Variant[]) => backend && run(() => backend!.setVariants(targetIds, vs));
  const setCosmetic = (on: boolean) => backend && run(() => backend!.setCosmetic(targetIds, on));
  const setMirrored = (on: boolean) => backend && run(() => backend!.setMirrored(targetIds, on));
  const rename = (name: string) => backend && activeId != null && run(() => backend!.rename(activeId!, name));

  async function removeSelected() {
    if (!backend || targetIds.length === 0) return;
    const ids = [...targetIds];
    const n = ids.length;
    if (!(await backend.confirm(`Remove ${n === 1 ? `"${active?.name}"` : `${n} plies`} from the project? (Undo brings ${n === 1 ? "it" : "them"} back.)`, "Remove"))) return;
    selection = [];
    activeId = null;
    await run(() => backend!.remove(ids), `Removed ${n} ${n === 1 ? "ply" : "plies"}`);
  }

  function reportImport(r: ImportReport) {
    const parts = [`${r.added} new`];
    if (r.changed) parts.push(`${r.changed} changed (check 0°)`);
    if (r.unchanged) parts.push(`${r.unchanged} unchanged`);
    toast(`Imported: ${parts.join(", ")}`);
    for (const w of r.warnings.slice(0, 6)) toast(w, "error");
    if (r.warnings.length > 6) toast(`…and ${r.warnings.length - 6} more warnings`, "error");
  }

  async function importFiles(paths?: string[]) {
    if (!backend) return;
    const files = paths ?? (await backend.pickDxfFiles());
    if (files.length === 0) return;
    const before = new Set(plies.map((p) => p.id));
    try {
      const { view: v, report } = await backend.importDxf(files);
      show(v);
      const added = v.plies.filter((p) => !before.has(p.id));
      if (added.length) {
        selection = [added[0].id];
        activeId = added[0].id;
      }
      reportImport(report);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async function reload() {
    if (!backend) return;
    try {
      const { view: v, report } = await backend.reload();
      show(v);
      reportImport(report);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async function guardUnsaved(): Promise<boolean> {
    return !view?.dirty || (await backend!.confirm("The project has unsaved changes. Discard them?", "Discard"));
  }

  async function newProject() {
    if (!backend || !(await guardUnsaved())) return;
    selection = [];
    activeId = null;
    await run(() => backend!.newProject());
  }

  async function openProject() {
    if (!backend || !(await guardUnsaved())) return;
    const path = await backend.pickProjectToOpen();
    if (!path) return;
    selection = [];
    activeId = null;
    await run(() => backend!.openProject(path), "Project opened");
  }

  async function save(as = false) {
    if (!backend || !view) return;
    let path: string | null = null;
    if (as || !view.path) {
      path = await backend.pickProjectToSave(`${view.name || "project"}.plyroll`);
      if (!path) return;
    }
    await run(() => backend!.saveProject(path), "Saved");
  }

  const undo = () => backend && view?.can_undo && run(() => backend!.undo());
  const redo = () => backend && view?.can_redo && run(() => backend!.redo());

  /** [ / Q turn clockwise, ] / E counter-clockwise; by key so it works on any layout. */
  function turnDir(e: KeyboardEvent): -1 | 0 | 1 {
    if (e.altKey && !["[", "]"].includes(e.key)) return 0;
    if (["[", "{", "q", "Q"].includes(e.key) || e.code === "BracketLeft") return -1;
    if (["]", "}", "e", "E"].includes(e.key) || e.code === "BracketRight") return 1;
    return 0;
  }

  // Preview the first ply angle when moving to another ply (indices differ per ply).
  let previewFor = -1;
  $effect(() => {
    if (activeId != null && activeId !== previewFor) {
      previewFor = activeId;
      if (preview >= 0) preview = 0;
    }
  });

  function onkeydown(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    const t = e.target as HTMLElement;
    const typing = t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT";
    if (mod) {
      const k = e.key.toLowerCase();
      const map: Record<string, () => void> = {
        s: () => save(e.shiftKey),
        o: openProject,
        i: () => importFiles(),
        n: newProject,
        z: () => (e.shiftKey ? redo() : undo()),
        y: redo,
        a: () => {
          if (typing) return;
          selection = listed.map((p) => p.id);
        },
      };
      if (map[k] && !(typing && (k === "z" || k === "y" || k === "a"))) {
        e.preventDefault();
        map[k]();
      }
      return;
    }
    if (typing || !active) {
      if (e.key === "Escape" && showHelp) showHelp = false;
      return;
    }
    const rules: Record<string, RefPick> = {
      x: { kind: "drawing-x" },
      y: { kind: "drawing-y" },
      l: { kind: "longest-edge" },
      b: { kind: "bounding-box" },
      f: { kind: "layer" },
    };
    const k = e.key.toLowerCase();
    if (rules[k] && !e.altKey) {
      e.preventDefault();
      pick(rules[k]);
    } else if (turnDir(e) !== 0) {
      e.preventDefault();
      pick({ kind: "rotate", delta: turnDir(e) * (e.shiftKey ? 15 : 1) });
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (targetIds.some((id) => plies.find((p) => p.id === id)?.status === "check")) pick({ kind: "confirm" });
      else nextTodo();
    } else if (k === "p") {
      pointsMode = !pointsMode;
    } else if (k === "n") {
      nextTodo();
    } else if (e.key === "Escape") {
      if (showHelp) showHelp = false;
      else if (pointsMode) pointsMode = false;
      else if (activeId != null) selection = [activeId];
    } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      e.preventDefault();
      step(1, e.shiftKey);
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      e.preventDefault();
      step(-1, e.shiftKey);
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      removeSelected();
    } else if (/^[1-9]$/.test(e.key)) {
      const i = Number(e.key) - 1;
      if (i < active.variants.length) preview = i;
    } else if (e.key === "0") {
      preview = active.variants.length > 1 ? -2 : -1;
    } else if (e.key === "?") {
      showHelp = !showHelp;
    } else if (k === "v") {
      editor?.fit();
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      backend = await createBackend();
      show(await backend.state());
      unlisten = await backend.onFileDrop((paths) => {
        const dxf = paths.filter((p) => /\.dxf$/i.test(p));
        const proj = paths.find((p) => /\.plyroll$/i.test(p));
        if (dxf.length) importFiles(dxf);
        else if (proj)
          guardUnsaved().then((ok) => {
            if (ok) {
              selection = [];
              activeId = null;
              run(() => backend!.openProject(proj), "Project opened");
            }
          });
        else toast("Drop DXF files or a .plyroll project", "error");
      });
      await backend.onCloseRequested(() => !!view?.dirty);
    })();
    return () => unlisten?.();
  });
</script>

<svelte:window {onkeydown} />

<div class="app">
  <header>
    <div class="brand">PlyRoll</div>
    <div class="group">
      <button onclick={newProject} title="New project (⌘N)">New</button>
      <button onclick={openProject} title="Open project (⌘O)">Open…</button>
      <button onclick={() => save(false)} title="Save (⌘S, ⇧⌘S = Save as)">Save</button>
    </div>
    <div class="group">
      <button class="primary" onclick={() => importFiles()} title="Import DXF files (⌘I) — or drop them on the window">Import DXF…</button>
      <button onclick={reload} disabled={!plies.length} title="Re-read every DXF; 0° references are kept, changed outlines are flagged">Reload DXFs</button>
    </div>
    <div class="group">
      <button onclick={undo} disabled={!view?.can_undo} title="Undo (⌘Z)">Undo</button>
      <button onclick={redo} disabled={!view?.can_redo} title="Redo (⇧⌘Z)">Redo</button>
    </div>
    <div class="title">
      {view?.name ?? ""}{#if view?.dirty}<span class="dirty" title="Unsaved changes">•</span>{/if}
      {#if backend?.kind === "mock"}<span class="mock">browser preview — nothing is saved</span>{/if}
    </div>
    {#if plies.length}
      <button class="ready" class:ok={view?.ready.ok} title={view?.ready.message} onclick={nextTodo}>
        {#if view?.ready.ok}Ready to nest{:else if todo.length}{todo.length} of {plies.length} need 0°{:else}Not ready{/if}
      </button>
    {/if}
    <button class="icon" onclick={() => (showHelp = !showHelp)} title="Keyboard shortcuts (?)">?</button>
  </header>

  {#if !view}
    <div class="empty"><p>Loading…</p></div>
  {:else if plies.length === 0}
    <div class="empty">
      <div class="drop">
        <h1>Set the fibre direction of every ply</h1>
        <p>Import the DXF outlines exported from CAD. Then mark which way 0° runs on each shape and list the ply angles you need.</p>
        <button class="primary big" onclick={() => importFiles()}>Import DXF files…</button>
        <p class="hint">or drop DXF files here · <button class="link" onclick={openProject}>open a project</button></p>
      </div>
    </div>
  {:else}
    <main>
      <nav class="list">
        <div class="filters" role="tablist">
          <button role="tab" aria-selected={filter === "all"} class:on={filter === "all"} onclick={() => (filter = "all")}>All {plies.length}</button>
          <button role="tab" aria-selected={filter === "todo"} class:on={filter === "todo"} onclick={() => (filter = "todo")}>To do {todo.length}</button>
          <label class="adv" title="After setting 0° on a ply that needed it, jump to the next one"><input type="checkbox" bind:checked={autoAdvance} /> auto-next</label>
        </div>
        <div class="grid">
          {#each listed as p (p.id)}
            <Thumb
              ply={p}
              selected={selection.includes(p.id)}
              active={p.id === activeId && selection.length > 1}
              onselect={(e) => select(p.id, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "replace")}
            />
          {:else}
            <p class="hint pad">Nothing left to do: every ply has a confirmed 0° reference.</p>
          {/each}
        </div>
      </nav>

      <section class="stage">
        {#if active}
          <div class="stage-bar">
            <b>{active.name}</b>
            <span class="chips">
              <span class="muted">Show fibre:</span>
              <button class:on={preview === -1} onclick={() => (preview = -1)}>0° axis only</button>
              {#each active.variants as v, i}
                <button class:on={preview === i} onclick={() => (preview = i)}>
                  <span class="swatch" style="background:{angleColor(v.angle_deg)}"></span>{variantLabel(v)}
                </button>
              {/each}
              {#if active.variants.length > 1}
                <button class:on={preview === -2} onclick={() => (preview = -2)}>All</button>
              {/if}
            </span>
            <span class="spacer"></span>
            {#if pointsMode}<span class="mode">Two-point mode · Esc to cancel</span>{/if}
            <button onclick={() => editor?.fit()} title="Fit to view (V, or double-click)">Fit</button>
          </div>
          <div class="canvas">
            <Editor bind:this={editor} ply={active} {pointsMode} {preview} onpick={pick} onpointsdone={() => (pointsMode = false)} />
            {#if active.status === "unset" && !pointsMode}
              <div class="banner">Click an edge that runs along <b>0°</b> · or press <kbd>X</kbd> <kbd>Y</kbd> <kbd>L</kbd> <kbd>B</kbd>{active.layer_ref_deg != null ? " F" : ""} · <kbd>P</kbd> for two points</div>
            {/if}
          </div>
        {/if}
      </section>

      {#if active}
        <Inspector
          {active}
          selected={selectedPlies.length ? selectedPlies : [active]}
          {pointsMode}
          onpick={pick}
          ontogglepoints={() => (pointsMode = !pointsMode)}
          onvariants={setVariants}
          oncosmetic={setCosmetic}
          onmirrored={setMirrored}
          onrename={rename}
          onremove={removeSelected}
        />
      {/if}
    </main>
  {/if}

  {#if showHelp}
    <div class="help" role="dialog" aria-label="Keyboard shortcuts">
      <h3>Keyboard</h3>
      <dl>
        <dt>click edge</dt><dd>that edge is 0°</dd>
        <dt>P</dt><dd>two-point mode (snaps to corners)</dd>
        <dt>X / Y</dt><dd>0° along drawing X / Y</dd>
        <dt>L / B / F</dt><dd>longest edge / bounding box / fibre layer</dd>
        <dt>[ ] or Q E</dt><dd>turn 0° by −1° / +1° (with ⇧: 15°)</dd>
        <dt>↵</dt><dd>confirm 0°, or go to the next ply to do</dd>
        <dt>N</dt><dd>next ply that needs 0°</dd>
        <dt>← → ↑ ↓</dt><dd>previous / next ply (⇧ extends the selection)</dd>
        <dt>⌘-click, ⇧-click</dt><dd>multi-select; rules then apply to all</dd>
        <dt>1–9 / 0</dt><dd>preview ply angle n / all</dd>
        <dt>V, double-click</dt><dd>fit view · wheel zooms · drag pans</dd>
        <dt>⌫</dt><dd>remove selected plies</dd>
        <dt>⌘Z / ⇧⌘Z</dt><dd>undo / redo</dd>
      </dl>
      <button onclick={() => (showHelp = false)}>Close</button>
    </div>
  {/if}

  <div class="toasts" aria-live="polite">
    {#each toasts as t (t.id)}
      <div class="toast {t.kind}">{t.text}</div>
    {/each}
  </div>
</div>

<style>
  .app {
    height: 100vh;
    display: grid;
    grid-template-rows: auto 1fr;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.02em;
    margin-right: 6px;
  }
  .group {
    display: flex;
    gap: 4px;
  }
  .title {
    flex: 1;
    min-width: 0;
    text-align: center;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dirty {
    color: var(--accent);
    margin-left: 4px;
  }
  .mock {
    margin-left: 10px;
    font-size: 11px;
    background: var(--warn-soft);
    color: var(--ink);
    padding: 2px 6px;
    border-radius: 4px;
  }
  .ready {
    background: var(--warn-soft);
    border-color: transparent;
  }
  .ready.ok {
    background: var(--ok-soft);
  }
  main {
    display: grid;
    grid-template-columns: 300px 1fr 330px;
    min-height: 0;
  }
  .list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
    background: var(--bg);
  }
  .filters {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 8px;
    border-bottom: 1px solid var(--line);
  }
  .filters button.on {
    background: var(--ink);
    color: var(--bg);
    border-color: var(--ink);
  }
  .adv {
    margin-left: auto;
    font-size: 12px;
    color: var(--muted);
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .grid {
    overflow-y: auto;
    padding: 8px;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    align-content: start;
  }
  .pad {
    grid-column: 1 / -1;
    padding: 8px;
  }
  .stage {
    display: grid;
    grid-template-rows: auto 1fr;
    min-width: 0;
    min-height: 0;
  }
  .stage-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    min-width: 0;
  }
  .chips {
    display: flex;
    gap: 4px;
    align-items: center;
    overflow-x: auto;
  }
  .chips button {
    font-size: 12px;
    padding: 3px 8px;
    display: flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
  }
  .chips button.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 2px;
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .mode {
    font-size: 12px;
    color: var(--accent);
    white-space: nowrap;
  }
  .canvas {
    position: relative;
    min-height: 0;
    background: var(--canvas);
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
    font-size: 13px;
    box-shadow: 0 2px 12px rgb(0 0 0 / 0.1);
    pointer-events: none;
    white-space: nowrap;
  }
  .empty {
    display: grid;
    place-items: center;
    background: var(--canvas);
  }
  .drop {
    max-width: 460px;
    text-align: center;
    padding: 36px;
    border: 2px dashed var(--line-strong);
    border-radius: 14px;
    background: var(--panel);
  }
  .drop h1 {
    font-size: 20px;
    margin: 0 0 10px;
  }
  .drop p {
    color: var(--muted);
    line-height: 1.5;
  }
  .big {
    font-size: 15px;
    padding: 9px 18px;
    margin: 8px 0;
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    padding: 0;
    text-decoration: underline;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
  }
  .icon {
    width: 28px;
    padding: 4px 0;
  }
  .help {
    position: fixed;
    right: 16px;
    top: 52px;
    z-index: 20;
    width: 380px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 14px 16px;
    box-shadow: 0 8px 30px rgb(0 0 0 / 0.18);
    font-size: 13px;
  }
  .help h3 {
    margin: 0 0 8px;
  }
  .help dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 5px 12px;
    margin: 0 0 12px;
  }
  .help dt {
    font-weight: 600;
    white-space: nowrap;
  }
  .help dd {
    margin: 0;
    color: var(--muted);
  }
  .toasts {
    position: fixed;
    bottom: 14px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: center;
    z-index: 30;
    pointer-events: none;
  }
  .toast {
    background: var(--ink);
    color: var(--bg);
    padding: 7px 12px;
    border-radius: 7px;
    font-size: 13px;
    max-width: 640px;
    white-space: pre-wrap;
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.2);
  }
  .toast.error {
    background: var(--bad);
    color: white;
  }
</style>
