<script lang="ts">
  import type { Hit, Summary } from "$lib/api";
  import { api } from "$lib/api";
  import { fmtDate, highlightParts } from "$lib/format";
  import { settings } from "$lib/settings.svelte";
  import { store } from "$lib/store.svelte";
  import { viewport } from "$lib/viewport.svelte";

  let {
    mode = "pane",
    onopen,
  }: {
    /** "pane" fills the window, "drawer" slides over the editor, "hidden" is off-screen. */
    mode?: "pane" | "drawer" | "hidden";
    /** Called after a row is opened, so a drawer can close itself. */
    onopen?: () => void;
  } = $props();

  const items = $derived(store.visible);

  function isSummary(item: Summary | Hit): item is Summary {
    return "updated" in item;
  }

  /** The row's right-click menu: which note it points at and where it is. */
  let menu = $state<{ id: string; x: number; y: number } | null>(null);

  /** The clipboard is not available everywhere (older WebKitGTK builds). */
  const canCopy = typeof navigator !== "undefined" && !!navigator.clipboard;

  /**
   * Open the menu on `item`. The note is selected first — the same thing the
   * GTK shell's menu does — so every item acts on the note it names (and
   * `select` flushes a pending edit).
   */
  async function openMenu(event: MouseEvent, item: Summary | Hit) {
    event.preventDefault();
    await store.select(item.id);
    menu = { id: item.id, x: event.clientX, y: event.clientY };
  }

  function closeMenu() {
    menu = null;
  }

  /** The note's file path, as the GTK shell's "Copy file path" reports it. */
  function notePath(id: string): string | null {
    const item = items.find((entry) => entry.id === id);
    const root = store.status.vaultPath;
    if (!item || !root) return null;
    return `${root}/${item.relPath}`;
  }

  async function copyNoteText(id: string) {
    try {
      const note = store.note?.id === id ? store.note : await api.get(id);
      await navigator.clipboard.writeText(note?.body ?? "");
      store.notify("Copied the note text");
    } catch (e) {
      store.error = String(e);
    }
  }

  async function copyFilePath(path: string) {
    try {
      await navigator.clipboard.writeText(path);
      store.notify("Copied the file path");
    } catch (e) {
      store.error = String(e);
    }
  }

  function onMenuKey(event: KeyboardEvent) {
    if (event.key === "Escape" && menu) closeMenu();
  }
</script>

<svelte:window onkeydown={onMenuKey} />

<aside
  class="sidebar"
  class:compact={settings.value.view.listDensity === "compact"}
  class:pane={mode === "pane"}
  class:drawer={mode === "drawer"}
  class:hidden={mode === "hidden"}
  style={!viewport.narrow && mode === "pane"
    ? `width: ${settings.value.view.sidebarWidth}px`
    : ""}
>
  {#if store.tags.length || store.folders.length || store.filtered}
    <div class="filters">
      {#if store.filtered}
        <div class="chipRow">
          {#if store.folderFilter}
            <button class="chip active" onclick={() => store.toggleFolder(store.folderFilter!)}>
              {store.folderFilter}<span class="x">×</span>
            </button>
          {/if}
          {#if store.tagFilter}
            <button class="chip active" onclick={() => store.toggleTag(store.tagFilter!)}>
              #{store.tagFilter}<span class="x">×</span>
            </button>
          {/if}
          <button class="clear" onclick={() => store.clearFilters()}>Clear</button>
        </div>
      {/if}
      {#if store.folders.length}
        <div class="chipRow">
          {#each store.folders as folder (folder.name)}
            <button
              class="chip"
              class:active={store.folderFilter === folder.name}
              onclick={() => store.toggleFolder(folder.name)}
            >
              {folder.name}<span class="count">{folder.count}</span>
            </button>
          {/each}
        </div>
      {/if}
      {#if store.tags.length}
        <div class="chipRow">
          {#each store.tags.slice(0, 18) as tag (tag.name)}
            <button
              class="chip"
              class:active={store.tagFilter === tag.name}
              onclick={() => store.toggleTag(tag.name)}
            >
              #{tag.name}<span class="count">{tag.count}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="listHead">
    <span>
      {#if store.hits !== null}
        Results ({store.hits.length})
      {:else}
        Notes ({store.notes.length})
      {/if}
    </span>
    {#if store.searching}
      <span class="searching">Searching…</span>
    {/if}
  </div>

  <div class="list">
    {#each items as item (item.id)}
      <button
        class="item"
        class:selected={item.id === store.selectedId}
        onclick={() => {
          void store.select(item.id);
          onopen?.();
        }}
        oncontextmenu={(event) => openMenu(event, item)}
      >
        <span class="titleRow">
          {#if item.pinned}<span class="pin">●</span>{/if}
          <span class="title">
            {#if !isSummary(item) && store.query.trim()}
              {#each highlightParts(item.title || "Untitled", store.query) as part, i (i)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}
            {:else}
              {item.title || "Untitled"}
            {/if}
          </span>
        </span>
        {#if !isSummary(item) && item.snippet}
          <span class="snippet">
            {#each highlightParts(item.snippet, store.query) as part, i (i)}{#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}
          </span>
        {/if}
        <span class="meta">
          {#if isSummary(item) && item.updated}{fmtDate(item.updated, settings.value.view.dateFormat)}{/if}
          {#if item.tags.length}
            {#if isSummary(item) && item.updated} · {/if}{item.tags.join(", ")}
          {/if}
        </span>
      </button>
    {/each}

    {#if items.length === 0}
      <div class="empty">
        {#if store.query.trim()}
          No notes match “{store.query.trim()}”.
        {:else if store.filtered}
          No notes match the current filter.
        {:else}
          No notes yet.<br />Press Ctrl+N to create one.
        {/if}
      </div>
    {/if}
  </div>
</aside>

{#if menu}
  {@const entry = menu}
  <div
    class="menuBackdrop"
    role="presentation"
    onclick={closeMenu}
    oncontextmenu={(event) => {
      event.preventDefault();
      closeMenu();
    }}
  ></div>
  <div class="menu" style="left: {entry.x}px; top: {entry.y}px" role="menu">
    <button class="menuItem" onclick={() => { closeMenu(); void store.duplicateNote(entry.id); }}>
      Duplicate
    </button>
    <button
      class="menuItem"
      onclick={() => {
        closeMenu();
        window.dispatchEvent(new CustomEvent("writan-rename"));
      }}
    >
      Rename…
    </button>
    <button class="menuItem" onclick={() => { closeMenu(); void store.togglePin(); }}>
      {store.note?.id === entry.id && store.note.pinned ? "Unpin" : "Pin"}
    </button>
    {#if canCopy}
      <button class="menuItem" onclick={() => { closeMenu(); void copyNoteText(entry.id); }}>
        Copy note text
      </button>
      {#if notePath(entry.id)}
        {@const path = notePath(entry.id)!}
        <button class="menuItem" onclick={() => { closeMenu(); void copyFilePath(path); }}>
          Copy file path
        </button>
      {/if}
    {/if}
    <hr />
    <button
      class="menuItem danger"
      onclick={() => {
        closeMenu();
        void store.removeNote();
      }}
    >
      Move to trash
    </button>
  </div>
{/if}

<style>
  .sidebar {
    width: 280px;
    min-width: 200px;
    max-width: 40vw;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }

  /* Phone layout: one pane at a time. "pane" is the whole window; "drawer"
     slides in over the editor; "hidden" stays mounted off-screen. */
  :global(:root[data-narrow]) .sidebar.pane {
    width: 100%;
    min-width: 0;
    max-width: none;
    border-right: none;
    padding-bottom: env(safe-area-inset-bottom);
  }

  .sidebar.drawer {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    z-index: 70;
    width: min(86vw, 340px);
    min-width: 0;
    max-width: none;
    padding-bottom: env(safe-area-inset-bottom);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.3);
  }

  .sidebar.hidden {
    display: none;
  }

  .filters {
    padding: 10px 12px 2px;
    border-bottom: 1px solid var(--border);
    max-height: 168px;
    overflow-y: auto;
  }

  .chipRow {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 8px;
  }

  .chip {
    font-size: 11.5px;
    padding: 3px 8px;
    border-radius: 999px;
    background: var(--bg-hover);
    color: var(--fg-muted);
    white-space: nowrap;
  }

  .chip:hover {
    color: var(--fg);
  }

  .chip.active {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .count {
    opacity: 0.65;
    margin-left: 2px;
  }

  .x {
    margin-left: 2px;
  }

  .clear {
    font-size: 11.5px;
    padding: 3px 6px;
    color: var(--fg-muted);
    text-decoration: underline;
  }

  .listHead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 14px 6px;
    font-size: 11.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    -webkit-user-select: none;
    user-select: none;
  }

  .searching {
    font-weight: 500;
    letter-spacing: 0.02em;
    text-transform: none;
    color: var(--accent);
  }

  .list {
    flex: 1;
    overflow-y: auto;
    padding: 0 8px 12px;
  }

  .item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    text-align: left;
    padding: 9px 10px;
    border-radius: var(--radius);
    margin-bottom: 2px;
  }

  .item:hover {
    background: var(--bg-hover);
  }

  .item.selected {
    background: var(--accent-soft);
  }

  .titleRow {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .pin {
    color: var(--accent);
    font-size: 9px;
    flex: none;
  }

  .title {
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .snippet {
    font-size: 12.5px;
    color: var(--fg-muted);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .meta {
    font-size: 11.5px;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  mark {
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: 3px;
    padding: 0 1px;
  }

  /* Phone layout: rows and chips grow to finger size. */
  :global(:root[data-narrow]) .item {
    padding: 12px;
    gap: 4px;
    min-height: 48px;
  }

  :global(:root[data-narrow]) .compact .item {
    min-height: 44px;
  }

  :global(:root[data-narrow]) .chip {
    padding: 7px 11px;
    font-size: 12.5px;
  }

  :global(:root[data-narrow]) .filters {
    max-height: 30vh;
  }

  .compact .item {
    padding: 5px 9px;
    gap: 2px;
  }

  .compact .snippet {
    -webkit-line-clamp: 1;
    line-clamp: 1;
  }

  .compact .meta {
    font-size: 11px;
  }

  .empty {
    padding: 24px 12px;
    text-align: center;
    color: var(--fg-muted);
    font-size: 13px;
    line-height: 1.6;
  }

  /* The note context menu: a fixed overlay at the pointer, dismissed by any
     click outside it (the backdrop) or Escape. */
  .menuBackdrop {
    position: fixed;
    inset: 0;
    z-index: 80;
  }

  .menu {
    position: fixed;
    z-index: 81;
    min-width: 180px;
    padding: 5px;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.25);
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .menu hr {
    width: 100%;
    border: none;
    border-top: 1px solid var(--border);
    margin: 4px 0;
  }

  .menuItem {
    text-align: left;
    padding: 7px 10px;
    border-radius: calc(var(--radius) - 3px);
    font-size: 13px;
  }

  .menuItem:hover {
    background: var(--bg-hover);
  }

  .menuItem.danger {
    color: var(--danger);
  }

  :global(:root[data-narrow]) .menuItem {
    padding: 13px 12px;
  }
</style>
