<script lang="ts">
  import { store } from "$lib/store.svelte";

  const vaultName = $derived(
    store.status.vaultPath
      ? (store.status.vaultPath.split(/[\\/]/).filter(Boolean).pop() ?? store.status.vaultPath)
      : "Choose folder…",
  );
</script>

<header class="topbar">
  <div class="brand">Writan</div>

  <input
    id="writan-search"
    class="search"
    placeholder="Search notes…"
    spellcheck="false"
    value={store.query}
    oninput={(e) => store.setQuery(e.currentTarget.value)}
  />

  <button class="btn" onclick={() => store.createNote()} title="New note (Ctrl+N)">+ New</button>
  <button
    class="btn"
    class:active={store.note?.pinned}
    disabled={!store.note}
    onclick={() => store.togglePin()}
    title="Pin note"
  >
    Pin
  </button>
  <button class="btn" onclick={() => store.toggleTheme()} title="Toggle theme">
    {store.theme === "dark" ? "Light" : "Dark"}
  </button>
  <button class="btn" onclick={() => store.toggleTrash()} title="Trash">Trash</button>
  <button class="btn vault" onclick={() => store.chooseVault()} title="Change notes folder">
    {vaultName}
  </button>
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    -webkit-user-select: none;
    user-select: none;
  }

  .brand {
    font-weight: 650;
    letter-spacing: 0.02em;
    padding-right: 6px;
    color: var(--accent);
  }

  .search {
    flex: 1;
    min-width: 120px;
    padding: 7px 12px;
    border-radius: var(--radius);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
  }

  .search:focus {
    border-color: var(--accent);
  }

  .search::placeholder {
    color: var(--fg-muted);
  }

  .btn {
    padding: 7px 12px;
    border-radius: var(--radius);
    border: 1px solid transparent;
    color: var(--fg);
    white-space: nowrap;
  }

  .btn:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .btn.active {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .vault {
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-muted);
  }
</style>
