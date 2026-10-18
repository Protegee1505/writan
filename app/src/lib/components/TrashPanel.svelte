<script lang="ts">
  import { fmtDate } from "$lib/format";
  import { store } from "$lib/store.svelte";
</script>

<div
  class="overlay"
  onclick={(e) => {
    if (e.target === e.currentTarget) store.trashOpen = false;
  }}
  onkeydown={(e) => {
    if (e.key === "Escape") store.trashOpen = false;
  }}
  role="presentation"
>
  <div class="panel">
    <header>
      <span>Trash</span>
      <div class="head-actions">
        <button
          class="quiet"
          onclick={() => store.emptyTrash()}
          disabled={store.trash.length === 0}
          title="Delete every trashed note forever">Empty Trash</button
        >
        <button class="close" onclick={() => (store.trashOpen = false)} title="Close">✕</button>
      </div>
    </header>

    <div class="entries">
      {#each store.trash as entry (entry.id)}
        <div class="row">
          <div class="info">
            <div class="t">{entry.title || "Untitled"}</div>
            <div class="m">{fmtDate(entry.deletedAt)} · {entry.originalPath}</div>
          </div>
          <div class="actions">
            <button onclick={() => store.restore(entry.id)}>Restore</button>
            <button class="danger" onclick={() => store.purge(entry.id)}>Delete forever</button>
          </div>
        </div>
      {/each}

      {#if store.trash.length === 0}
        <div class="empty">Trash is empty.</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .panel {
    width: min(640px, 92vw);
    max-height: 76vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.28);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    font-weight: 600;
  }

  .close {
    padding: 4px 8px;
    border-radius: 8px;
    color: var(--fg-muted);
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .quiet {
    padding: 4px 10px;
    border-radius: 8px;
    color: var(--fg-muted);
    font-weight: 500;
  }

  .quiet:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--fg);
  }

  .quiet:disabled {
    opacity: 0.5;
  }

  .close:hover {
    background: var(--bg-hover);
  }

  .entries {
    overflow-y: auto;
    padding: 8px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: var(--radius);
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .info {
    flex: 1;
    min-width: 0;
  }

  .t {
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .m {
    font-size: 12px;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .actions {
    display: flex;
    gap: 6px;
    flex: none;
  }

  .actions button {
    padding: 5px 10px;
    border-radius: 8px;
    border: 1px solid var(--border);
  }

  .actions button:hover {
    background: var(--bg);
  }

  .actions .danger {
    color: var(--danger);
  }

  .empty {
    padding: 32px 12px;
    text-align: center;
    color: var(--fg-muted);
  }
</style>
