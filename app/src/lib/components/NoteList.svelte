<script lang="ts">
  import type { Hit, Summary } from "$lib/api";
  import { fmtDate } from "$lib/format";
  import { store } from "$lib/store.svelte";

  const items = $derived(store.visible);

  function isSummary(item: Summary | Hit): item is Summary {
    return "updated" in item;
  }
</script>

<aside class="sidebar">
  <div class="listHead">
    {#if store.hits !== null}
      Results ({store.hits.length})
    {:else}
      Notes ({store.notes.length})
    {/if}
  </div>

  <div class="list">
    {#each items as item (item.id)}
      <button
        class="item"
        class:selected={item.id === store.selectedId}
        onclick={() => store.select(item.id)}
      >
        <span class="titleRow">
          {#if item.pinned}<span class="pin">●</span>{/if}
          <span class="title">{item.title || "Untitled"}</span>
        </span>
        {#if !isSummary(item) && item.snippet}
          <span class="snippet">{item.snippet}</span>
        {/if}
        <span class="meta">
          {#if isSummary(item) && item.updated}{fmtDate(item.updated)}{/if}
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
        {:else}
          No notes yet.<br />Press Ctrl+N to create one.
        {/if}
      </div>
    {/if}
  </div>
</aside>

<style>
  .sidebar {
    width: 290px;
    min-width: 220px;
    max-width: 40vw;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }

  .listHead {
    padding: 10px 14px 6px;
    font-size: 11.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    -webkit-user-select: none;
    user-select: none;
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

  .empty {
    padding: 24px 12px;
    text-align: center;
    color: var(--fg-muted);
    font-size: 13px;
    line-height: 1.6;
  }
</style>
