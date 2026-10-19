<script lang="ts">
  import { openPath } from "@tauri-apps/plugin-opener";
  import {
    settings,
    SECTIONS,
    OPTIONS,
    ACCENTS,
    DEFAULT_SETTINGS,
    getPath,
    type OptionSpec,
    type SectionId,
  } from "$lib/settings.svelte";
  import {
    KEY_ACTIONS,
    effectiveBinding,
    bindingFromEvent,
    bindingConflict,
    isOverridden,
    isReserved,
    actionLabel,
    type ActionId,
  } from "$lib/keymap";
  import { store } from "$lib/store.svelte";
  import SyncSection from "$lib/components/SyncSection.svelte";

  let { open = $bindable(false), focusPath = null }: {
    open: boolean;
    focusPath: string | null;
  } = $props();

  let filter = $state("");
  let section = $state<SectionId>("appearance");
  let recording = $state<ActionId | null>(null);
  let recordError = $state<string | null>(null);
  let resetArmed = $state(false);
  let filterEl = $state<HTMLInputElement | undefined>();
  let flashPath = $state<string | null>(null);
  let wasOpen = false;
  let armTimer: ReturnType<typeof setTimeout> | null = null;
  let flashTimer: ReturnType<typeof setTimeout> | null = null;

  const filtered = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return [];
    return OPTIONS.filter(
      (spec) =>
        !spec.linuxOnly &&
        [spec.label, spec.description ?? "", ...(spec.keywords ?? [])]
          .join(" ")
          .toLowerCase()
          .includes(q),
    );
  });

  function optionsOf(id: SectionId): OptionSpec[] {
    return OPTIONS.filter((spec) => spec.section === id && !spec.linuxOnly);
  }

  function valueOf(path: string): unknown {
    return getPath(settings.value, path);
  }

  function isChanged(path: string): boolean {
    return JSON.stringify(valueOf(path)) !== JSON.stringify(getPath(DEFAULT_SETTINGS, path));
  }

  function numOf(path: string): number {
    const v = valueOf(path);
    return typeof v === "number" ? v : 0;
  }

  function prettyNum(v: number): string {
    return Number.isInteger(v) ? String(v) : v.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  }

  function scrollToPath(path: string) {
    const el = document.getElementById(`set-${path}`);
    el?.scrollIntoView({ block: "center" });
    flashPath = path;
    if (flashTimer) clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flashPath = null), 900);
  }

  $effect(() => {
    if (open && !wasOpen) {
      wasOpen = true;
      filter = "";
      recording = null;
      recordError = null;
      if (focusPath) {
        const sec = focusPath.split(".")[0] as SectionId;
        if (SECTIONS.some((s) => s.id === sec)) section = sec;
        requestAnimationFrame(() => scrollToPath(focusPath));
      } else {
        requestAnimationFrame(() => filterEl?.focus());
      }
    } else if (!open && wasOpen) {
      wasOpen = false;
      settings.flush();
    }
  });

  // Capture-phase recorder: while recording, the next combo becomes the binding.
  $effect(() => {
    if (!recording) return;
    const id = recording;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        recording = null;
        recordError = null;
        return;
      }
      if (e.key === "Backspace" || e.key === "Delete") {
        settings.set(`shortcuts.${id}`, null);
        recording = null;
        recordError = null;
        return;
      }
      const binding = bindingFromEvent(e);
      if (!binding) {
        recordError = "Include Ctrl or Alt in the shortcut";
        return;
      }
      if (isReserved(binding)) {
        recordError = `${binding} is reserved for text editing`;
        return;
      }
      const conflict = bindingConflict(id, binding);
      if (conflict) {
        recordError = `${binding} is already used by “${actionLabel(conflict)}”`;
        return;
      }
      settings.set(`shortcuts.${id}`, binding);
      recording = null;
      recordError = null;
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  function onWindowKey(e: KeyboardEvent) {
    if (!open || recording) return;
    if (e.key === "Escape") {
      e.preventDefault();
      open = false;
    }
  }

  function disarm() {
    if (armTimer) clearTimeout(armTimer);
    armTimer = setTimeout(() => (resetArmed = false), 3000);
  }

  function resetAll() {
    if (!resetArmed) {
      resetArmed = true;
      disarm();
      return;
    }
    resetArmed = false;
    settings.resetAll();
    store.notify("All settings were reset to their defaults.");
  }
</script>

<svelte:window onkeydown={onWindowKey} />

{#if open}
  <div class="overlay">
    <button class="backdrop" aria-label="Close settings" onclick={() => (open = false)}></button>
    <div class="panel">
      <header>
        <h2>Settings</h2>
        <input
          bind:this={filterEl}
          bind:value={filter}
          class="filter"
          placeholder="Search settings…"
          spellcheck="false"
        />
        <button class="close" onclick={() => (open = false)} aria-label="Close">×</button>
      </header>

      <div class="body">
        {#if filter.trim()}
          <div class="rows search-results">
            {#each filtered as spec (spec.path)}
              {@render optionRow(spec, true)}
            {/each}
            {#if filtered.length === 0}
              <div class="empty">No matching settings</div>
            {/if}
          </div>
        {:else}
          <nav class="tabs">
            {#each SECTIONS as entry (entry.id)}
              <button
                class="tab"
                class:active={entry.id === section}
                onclick={() => (section = entry.id)}
              >
                {entry.label}
              </button>
            {/each}
          </nav>

          <div class="content">
            {#if section === "shortcuts"}
              <p class="hint">
                Click Change, then press the keys. Backspace unbinds, Esc cancels. A shortcut
                cannot clash with another action or with text editing keys.
              </p>
              <div class="rows">
                {#each KEY_ACTIONS as action (action.id)}
                  <div class="row" class:flash={flashPath === `shortcuts.${action.id}`} id={`set-shortcuts.${action.id}`}>
                    <div class="meta">
                      <span class="label">{action.label}</span>
                      {#if isOverridden(action.id)}
                        <button
                          class="link"
                          onclick={() => settings.unset(`shortcuts.${action.id}`)}
                        >
                          Reset
                        </button>
                      {/if}
                    </div>
                    <div class="control shortcut">
                      {#if recording === action.id}
                        <span class="recording">Press keys…</span>
                      {:else if effectiveBinding(action.id)}
                        <kbd>{effectiveBinding(action.id)}</kbd>
                      {:else}
                        <span class="unset">Not set</span>
                      {/if}
                      <button
                        class="btn"
                        class:recording={recording === action.id}
                        onclick={() => {
                          recording = recording === action.id ? null : action.id;
                          recordError = null;
                        }}
                      >
                        {recording === action.id ? "Cancel" : "Change"}
                      </button>
                    </div>
                    {#if recording === action.id && recordError}
                      <div class="row-error">{recordError}</div>
                    {/if}
                  </div>
                {/each}
              </div>
            {:else if section === "sync"}
              <SyncSection />
            {:else if section === "advanced"}
              <div class="rows">
                <div class="row column">
                  <div class="meta">
                    <span class="label">Data folder</span>
                    <span class="desc">
                      Config, index, trash and keys live here. Never inside the vault.
                    </span>
                  </div>
                  <div class="control">
                    <code class="path">{store.status.dataDir || "…"}</code>
                    {#if !store.android}
                      <button
                        class="btn"
                        disabled={!store.status.dataDir}
                        onclick={() => store.status.dataDir && void openPath(store.status.dataDir)}
                      >
                        Open
                      </button>
                    {/if}
                  </div>
                </div>
                <div class="row column">
                  <div class="meta">
                    <span class="label">Search index</span>
                    <span class="desc">
                      Rebuildable from the Markdown files at any time.
                    </span>
                  </div>
                  <div class="control">
                    <button class="btn" onclick={() => void store.rebuildIndex()}>
                      Rebuild now
                    </button>
                  </div>
                </div>
                <div class="row column">
                  <div class="meta">
                    <span class="label">Reset all settings</span>
                    <span class="desc">Back to defaults. Notes are not touched.</span>
                  </div>
                  <div class="control">
                    <button class="btn danger" class:armed={resetArmed} onclick={resetAll}>
                      {resetArmed ? "Click again to confirm" : "Reset settings"}
                    </button>
                  </div>
                </div>
              </div>
            {:else}
              <div class="rows">
                {#each optionsOf(section) as spec (spec.path)}
                  {@render optionRow(spec, false)}
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

{#snippet optionRow(spec: OptionSpec, showSection: boolean)}
  <div class="row" class:flash={flashPath === spec.path} id={`set-${spec.path}`}>
    <div class="meta">
      <span class="label">
        {spec.label}
      </span>
      {#if spec.description}
        <span class="desc">{spec.description}</span>
      {/if}
      {#if showSection}
        <span class="section-tag">
          {SECTIONS.find((s) => s.id === spec.section)?.label}
        </span>
      {/if}
    </div>
    <div class="control">
      {#if spec.control.kind === "toggle"}
        <button
          class="switch"
          class:on={valueOf(spec.path) === true}
          role="switch"
          aria-label={spec.label}
          aria-checked={valueOf(spec.path) === true}
          onclick={() => settings.set(spec.path, valueOf(spec.path) !== true)}
        >
          <span class="knob"></span>
        </button>
      {:else if spec.control.kind === "select"}
        <select
          value={String(valueOf(spec.path))}
          onchange={(e) => {
            const choice = spec.control.kind === "select"
              ? spec.control.choices.find((c) => String(c.value) === e.currentTarget.value)
              : undefined;
            if (choice) settings.set(spec.path, choice.value);
          }}
        >
          {#each spec.control.choices as choice (String(choice.value))}
            <option value={String(choice.value)}>{choice.label}</option>
          {/each}
        </select>
      {:else if spec.control.kind === "slider"}
        <input
          type="range"
          min={spec.control.min}
          max={spec.control.max}
          step={spec.control.step}
          value={numOf(spec.path)}
          oninput={(e) => settings.set(spec.path, Number(e.currentTarget.value))}
        />
        <span class="value">{prettyNum(numOf(spec.path))}{spec.control.unit ?? ""}</span>
      {:else if spec.control.kind === "accent"}
        <div class="swatches">
          {#each ACCENTS as accent (accent.id)}
            <button
              class="swatch"
              class:active={settings.value.appearance.accent === accent.id}
              style={`--swatch: ${accent.light}`}
              title={accent.label}
              aria-label={accent.label}
              onclick={() => settings.set("appearance.accent", accent.id)}
            ></button>
          {/each}
        </div>
      {/if}
      {#if isChanged(spec.path) && spec.control.kind !== "accent"}
        <button class="link" onclick={() => settings.set(spec.path, getPath(DEFAULT_SETTINGS, spec.path))}>
          Reset
        </button>
      {/if}
    </div>
  </div>
{/snippet}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 8vh;
    z-index: 75;
  }

  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.32);
    border: none;
    border-radius: 0;
    cursor: default;
  }

  .panel {
    position: relative;
    width: min(720px, 92vw);
    height: min(560px, 78vh);
    display: flex;
    flex-direction: column;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }

  .filter {
    flex: 1;
    padding: 7px 12px;
    border-radius: var(--radius);
    background: var(--bg);
    border: 1px solid var(--border);
  }

  .filter:focus {
    border-color: var(--accent);
  }

  .close {
    font-size: 18px;
    line-height: 1;
    padding: 6px 10px;
    border-radius: var(--radius);
    color: var(--fg-muted);
  }

  .close:hover {
    background: var(--bg-hover);
    color: var(--fg);
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .tabs {
    flex: none;
    width: 150px;
    padding: 10px;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
  }

  .tab {
    text-align: left;
    padding: 8px 10px;
    border-radius: 8px;
    color: var(--fg-muted);
  }

  .tab:hover {
    background: var(--bg-hover);
  }

  .tab.active {
    color: var(--accent);
    background: var(--accent-soft);
    font-weight: 600;
  }

  .content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 10px 14px 20px;
  }

  .rows {
    display: flex;
    flex-direction: column;
  }

  .search-results {
    padding-top: 8px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 10px 8px;
    border-bottom: 1px solid var(--border);
    border-radius: 6px;
  }

  .row:last-child {
    border-bottom: none;
  }

  .row.flash {
    background: var(--accent-soft);
  }

  .row.column {
    align-items: flex-start;
    flex-direction: column;
    gap: 8px;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .meta .label {
    font-weight: 550;
  }

  .desc {
    font-size: 12px;
    color: var(--fg-muted);
  }

  .section-tag {
    font-size: 11px;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .control {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }

  .control.shortcut {
    gap: 10px;
  }

  kbd {
    font-family: var(--font-mono);
    font-size: 11.5px;
    padding: 3px 7px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 6px;
    background: var(--bg);
    color: var(--fg);
  }

  .unset,
  .recording {
    font-size: 12px;
    color: var(--fg-muted);
  }

  .recording {
    color: var(--accent);
    border: 1px dashed var(--accent);
    border-radius: 6px;
    padding: 3px 8px;
  }

  .row-error {
    width: 100%;
    font-size: 12px;
    color: var(--danger);
  }

  select {
    padding: 6px 8px;
    border-radius: 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--fg);
    min-width: 150px;
  }

  input[type="range"] {
    width: 160px;
    accent-color: var(--accent);
  }

  .value {
    min-width: 46px;
    text-align: right;
    font-size: 12.5px;
    color: var(--fg-muted);
  }

  .switch {
    width: 40px;
    height: 22px;
    border-radius: 999px;
    background: var(--border);
    position: relative;
    transition: background 120ms ease;
  }

  .switch.on {
    background: var(--accent);
  }

  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #fff;
    transition: transform 120ms ease;
  }

  .switch.on .knob {
    transform: translateX(18px);
  }

  .swatches {
    display: flex;
    gap: 6px;
  }

  .swatch {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--swatch);
    border: 2px solid transparent;
  }

  .swatch.active {
    border-color: var(--fg);
    box-shadow: 0 0 0 2px var(--bg-elevated);
  }

  .btn {
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--border);
    color: var(--fg);
    white-space: nowrap;
  }

  .btn:hover {
    background: var(--bg-hover);
  }

  .btn.recording {
    border-color: var(--accent);
    color: var(--accent);
  }

  .btn.danger {
    color: var(--danger);
  }

  .btn.armed {
    background: var(--danger);
    color: #fff;
    border-color: var(--danger);
  }

  .link {
    font-size: 12px;
    color: var(--accent);
    padding: 2px 4px;
    border-radius: 6px;
  }

  .link:hover {
    background: var(--accent-soft);
  }

  .path {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--fg-muted);
    word-break: break-all;
    max-width: 420px;
  }

  .hint {
    margin: 4px 8px 8px;
    font-size: 12.5px;
    color: var(--fg-muted);
    line-height: 1.5;
  }

  .empty {
    padding: 24px 12px;
    text-align: center;
    color: var(--fg-muted);
  }

  /* Phone layout: a full-screen sheet with the sections as a horizontal
     strip, so rows keep their width instead of being squeezed. */
  :global(:root[data-narrow]) .overlay {
    padding-top: 0;
    align-items: stretch;
  }

  :global(:root[data-narrow]) .panel {
    width: 100%;
    height: 100%;
    border: none;
    border-radius: 0;
    box-shadow: none;
  }

  :global(:root[data-narrow]) header {
    gap: 8px;
    padding: calc(10px + env(safe-area-inset-top)) 12px 10px;
  }

  :global(:root[data-narrow]) .body {
    flex-direction: column;
  }

  :global(:root[data-narrow]) .tabs {
    width: auto;
    flex-direction: row;
    gap: 6px;
    padding: 8px 10px;
    border-right: none;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
    overflow-y: hidden;
  }

  :global(:root[data-narrow]) .tab {
    padding: 10px 12px;
    white-space: nowrap;
  }

  :global(:root[data-narrow]) .content {
    padding: 6px 12px calc(24px + env(safe-area-inset-bottom));
  }

  :global(:root[data-narrow]) .row {
    flex-wrap: wrap;
    gap: 10px;
    padding: 12px 6px;
  }

  :global(:root[data-narrow]) .close {
    padding: 10px 12px;
  }

  :global(:root[data-narrow]) select {
    min-width: 0;
  }

  :global(:root[data-narrow]) input[type="range"] {
    width: 130px;
  }
</style>
