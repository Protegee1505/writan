<script lang="ts">
  import Logo from "./Logo.svelte";
  import { store } from "$lib/store.svelte";
  import { settings } from "$lib/settings.svelte";
  import { effectiveBinding, type ActionId } from "$lib/keymap";

  let { onsettings }: { onsettings: () => void } = $props();

  const vaultName = $derived(
    store.status.vaultPath
      ? (store.status.vaultPath.split(/[\\/]/).filter(Boolean).pop() ?? store.status.vaultPath)
      : "Choose folder…",
  );

  function keyHint(id: ActionId): string {
    const binding = effectiveBinding(id);
    return binding ? ` (${binding})` : "";
  }
</script>

<header class="topbar">
  <div class="brand" title="Writan">
    <Logo size={22} />
  </div>

  <div class="searchWrap">
    <input
      id="writan-search"
      class="search"
      placeholder="Search notes…"
      spellcheck="false"
      value={store.query}
      oninput={(e) => store.setQuery(e.currentTarget.value)}
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          store.commitQuery();
        } else if (e.key === "Escape") {
          store.clearQuery();
          e.currentTarget.blur();
        }
      }}
    />
    {#if store.searching}
      <span class="spin" title="Searching…" aria-label="Searching"></span>
    {/if}
    {#if store.query}
      <button
        class="clear"
        title="Clear search"
        aria-label="Clear search"
        onclick={() => store.clearQuery()}
      >
        ×
      </button>
    {/if}
  </div>

  <button class="btn" onclick={() => store.createNote()} title={`New note${keyHint("new-note")}`}>
    + New
  </button>
  <button
    class="btn"
    class:active={store.note?.pinned}
    disabled={!store.note}
    onclick={() => store.togglePin()}
    title={`Pin / unpin note${keyHint("pin-note")}`}
  >
    Pin
  </button>
  <button
    class="btn"
    onclick={() => store.toggleTheme()}
    title={`Switch to ${settings.dark ? "light" : "dark"} theme${keyHint("toggle-theme")}`}
  >
    {settings.dark ? "Light" : "Dark"}
  </button>
  <button class="btn" onclick={() => store.toggleTrash()} title={`Trash${keyHint("toggle-trash")}`}>
    Trash
  </button>
  <button
    class="btn gear"
    onclick={onsettings}
    title={`Settings${keyHint("open-settings")}`}
    aria-label="Settings"
  >
    ⚙
  </button>
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
    display: flex;
    align-items: center;
    padding-right: 4px;
  }

  .searchWrap {
    position: relative;
    flex: 1;
    min-width: 120px;
    display: flex;
    align-items: center;
  }

  .search {
    flex: 1;
    min-width: 120px;
    padding: 7px 36px 7px 12px;
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

  .spin {
    position: absolute;
    right: 32px;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid var(--accent);
    border-top-color: transparent;
    animation: spin 0.7s linear infinite;
    pointer-events: none;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .clear {
    position: absolute;
    right: 6px;
    width: 22px;
    height: 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    font-size: 15px;
    line-height: 1;
    color: var(--fg-muted);
  }

  .clear:hover {
    background: var(--bg-hover);
    color: var(--fg);
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

  .gear {
    font-size: 15px;
    padding: 5px 10px;
  }

  .vault {
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-muted);
  }
</style>
