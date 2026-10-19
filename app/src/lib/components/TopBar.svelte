<script lang="ts">
  import Logo from "./Logo.svelte";
  import { store } from "$lib/store.svelte";
  import { settings } from "$lib/settings.svelte";
  import { viewport } from "$lib/viewport.svelte";
  import { effectiveBinding, type ActionId } from "$lib/keymap";

  let {
    onsettings,
    ondrawer = () => {},
    drawerOpen = false,
  }: {
    onsettings: () => void;
    /** Opens or closes the note-list drawer (phone layout only). */
    ondrawer?: () => void;
    drawerOpen?: boolean;
  } = $props();

  /** The phone overflow menu: everything the desktop bar shows, in one popup. */
  let menuOpen = $state(false);

  const vaultName = $derived(
    store.status.vaultPath
      ? (store.status.vaultPath.split(/[\\/]/).filter(Boolean).pop() ?? store.status.vaultPath)
      : "Choose folder…",
  );

  function keyHint(id: ActionId): string {
    const binding = effectiveBinding(id);
    return binding ? ` (${binding})` : "";
  }

  function closeMenu() {
    menuOpen = false;
  }
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape" && menuOpen) closeMenu();
  }}
/>

<header class="topbar">
  {#if viewport.narrow}
    {#if store.note}
      <button
        class="btn icon"
        class:active={drawerOpen}
        onclick={ondrawer}
        title="Notes"
        aria-label="Notes list"
        aria-expanded={drawerOpen}
      >
        ☰
      </button>
    {/if}
  {:else}
    <div class="brand" title="Writan">
      <Logo size={22} />
    </div>
  {/if}

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

  {#if viewport.narrow}
    <button
      class="btn icon"
      onclick={() => store.createNote()}
      title="New note"
      aria-label="New note"
    >
      +
    </button>
    <div class="overflow">
      <button
        class="btn icon"
        class:active={menuOpen}
        onclick={() => (menuOpen = !menuOpen)}
        title="More actions"
        aria-label="More actions"
        aria-expanded={menuOpen}
      >
        ⋮
      </button>
      {#if menuOpen}
        <div class="menuBackdrop" role="presentation" onclick={closeMenu}></div>
        <div class="menu" role="menu">
          <button
            class="menuItem"
            disabled={!store.note}
            onclick={() => {
              closeMenu();
              void store.togglePin();
            }}
          >
            {store.note?.pinned ? "Unpin note" : "Pin note"}
          </button>
          <button
            class="menuItem"
            disabled={!store.note}
            onclick={() => {
              closeMenu();
              void store.duplicateNote();
            }}
          >
            Duplicate note
          </button>
          <button
            class="menuItem"
            onclick={() => {
              closeMenu();
              store.toggleTheme();
            }}
          >
            {settings.dark ? "Light theme" : "Dark theme"}
          </button>
          <button
            class="menuItem"
            onclick={() => {
              closeMenu();
              void store.toggleTrash();
            }}
          >
            Trash
          </button>
          <button
            class="menuItem"
            onclick={() => {
              closeMenu();
              onsettings();
            }}
          >
            Settings
          </button>
          {#if !store.android}
            <button
              class="menuItem"
              onclick={() => {
                closeMenu();
                void store.chooseVault();
              }}
            >
              Change notes folder…
            </button>
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    <button
      class="btn"
      onclick={() => store.createNote()}
      title={`New note${keyHint("new-note")}`}
    >
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
    <button
      class="btn"
      onclick={() => store.toggleTrash()}
      title={`Trash${keyHint("toggle-trash")}`}
    >
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
    {#if !store.android}
      <button class="btn vault" onclick={() => store.chooseVault()} title="Change notes folder">
        {vaultName}
      </button>
    {/if}
  {/if}
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: calc(8px + env(safe-area-inset-top)) 12px 8px;
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

  .icon {
    min-width: 34px;
    text-align: center;
    font-size: 15px;
    line-height: 1;
  }

  .overflow {
    position: relative;
    display: flex;
    align-items: center;
  }

  .menuBackdrop {
    position: fixed;
    inset: 0;
    z-index: 79;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 80;
    min-width: 200px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.24);
  }

  .menuItem {
    display: block;
    width: 100%;
    text-align: left;
    padding: 9px 10px;
    border-radius: 8px;
    font-size: 13.5px;
    white-space: nowrap;
  }

  .menuItem:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .menuItem:disabled {
    opacity: 0.45;
    cursor: default;
  }

  /* Phone layout: bigger tap targets and a bar that clears the status bar. */
  :global(:root[data-narrow]) .topbar {
    gap: 6px;
    padding: calc(6px + env(safe-area-inset-top)) 10px 6px;
  }

  :global(:root[data-narrow]) .btn {
    padding: 11px 12px;
  }

  :global(:root[data-narrow]) .icon {
    min-width: 42px;
    font-size: 17px;
  }

  :global(:root[data-narrow]) .search {
    padding: 11px 36px 11px 12px;
  }

  :global(:root[data-narrow]) .menuItem {
    padding: 14px 12px;
  }
</style>
