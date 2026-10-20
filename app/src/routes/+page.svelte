<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { store } from "$lib/store.svelte";
  import { viewport } from "$lib/viewport.svelte";
  import { matchesAction } from "$lib/keymap";
  import TopBar from "$lib/components/TopBar.svelte";
  import NoteList from "$lib/components/NoteList.svelte";
  import EditorPane from "$lib/components/EditorPane.svelte";
  import TrashPanel from "$lib/components/TrashPanel.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";
  import SettingsPanel from "$lib/components/SettingsPanel.svelte";
  import Logo from "$lib/components/Logo.svelte";

  let unlisten: (() => void) | null = null;
  let paletteOpen = $state(false);
  let settingsOpen = $state(false);
  let settingsFocus = $state<string | null>(null);
  /** On a phone the list slides over the editor as a drawer (see `listMode`). */
  let drawerOpen = $state(false);

  // One pane at a time on a phone: the editor owns the window whenever a note
  // is open, and the list is either that pane (no note) or an overlay drawer.
  const showListPane = $derived(!viewport.narrow || !store.note);
  const showDrawer = $derived(viewport.narrow && Boolean(store.note) && drawerOpen);
  const listMode = $derived<"pane" | "drawer" | "hidden">(
    showListPane ? "pane" : showDrawer ? "drawer" : "hidden",
  );

  // The drawer only exists on top of an open note in a narrow window.
  $effect(() => {
    if (!viewport.narrow || !store.note) drawerOpen = false;
  });

  onMount(() => {
    void (async () => {
      await store.init();
      unlisten = await listen<string[]>("vault-changed", (event) => {
        void store.onExternalChange(event.payload);
      });
    })();
    return () => {
      unlisten?.();
      unlisten = null;
    };
  });

  function openSettings(path: string | null = null) {
    settingsFocus = path;
    settingsOpen = true;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && drawerOpen) {
      drawerOpen = false;
      return;
    }
    if (matchesAction(e, "command-palette")) {
      e.preventDefault();
      paletteOpen = true;
    } else if (matchesAction(e, "new-note") && store.status.vaultPath) {
      e.preventDefault();
      void store.createNote();
    } else if (matchesAction(e, "focus-search")) {
      e.preventDefault();
      (document.getElementById("writan-search") as HTMLInputElement | null)?.focus();
    } else if (matchesAction(e, "save-note")) {
      void store.saveNow();
    } else if (matchesAction(e, "open-settings")) {
      e.preventDefault();
      openSettings(null);
    } else if (matchesAction(e, "toggle-theme")) {
      e.preventDefault();
      store.toggleTheme();
    } else if (matchesAction(e, "toggle-trash")) {
      e.preventDefault();
      void store.toggleTrash();
    } else if (matchesAction(e, "pin-note")) {
      e.preventDefault();
      void store.togglePin();
    } else if (matchesAction(e, "duplicate-note") && store.status.vaultPath) {
      e.preventDefault();
      void store.duplicateNote();
    } else if (matchesAction(e, "sync-now") && store.status.vaultPath) {
      e.preventDefault();
      void store.syncNow();
    } else if (matchesAction(e, "export-backup") && store.status.vaultPath && !store.android) {
      e.preventDefault();
      void store.exportBackup();
    } else if (matchesAction(e, "import-notes") && store.status.vaultPath && !store.android) {
      e.preventDefault();
      void store.importBackup();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if !store.ready}
  <div class="splash">
    <Logo size={40} />
    <span>Writan</span>
  </div>
{:else if store.status.vaultPath === null}
  <main class="welcome">
    <div class="card">
      <Logo size={44} />
      <h1>Writan</h1>
      <p class="tagline">A fast, local-first notebook. Your notes are plain Markdown files in a folder you own.</p>
      {#if store.android}
        <button class="cta" onclick={() => store.useDefaultVault()}>Use this device's storage</button>
        <p class="hint">Writan keeps its notes folder in the app's own storage here — plain Markdown, no account.</p>
      {:else}
        <button class="cta" onclick={() => store.chooseVault()}>Choose notes folder</button>
        <p class="hint">Nothing leaves your machine.</p>
      {/if}
    </div>
  </main>
{:else}
  <div class="app">
    <TopBar
      onsettings={() => openSettings(null)}
      drawerOpen={showDrawer}
      ondrawer={() => (drawerOpen = !drawerOpen)}
    />
    <div class="main">
      <NoteList mode={listMode} onopen={() => (drawerOpen = false)} />
      {#if !viewport.narrow || store.note}
        <EditorPane />
      {/if}
      {#if showDrawer}
        <div class="scrim" role="presentation" onclick={() => (drawerOpen = false)}></div>
      {/if}
    </div>
    {#if store.trashOpen}
      <TrashPanel />
    {/if}
    <CommandPalette
      bind:open={paletteOpen}
      onclose={() => (paletteOpen = false)}
      onsettings={openSettings}
    />
    <SettingsPanel bind:open={settingsOpen} focusPath={settingsFocus} />
    {#if store.notice}
      <div class="toast notice">
        <span>{store.notice}</span>
        <button onclick={() => (store.notice = null)}>Dismiss</button>
      </div>
    {/if}
    {#if store.error}
      <div class="toast">
        <span>{store.error}</span>
        <button onclick={() => (store.error = null)}>Dismiss</button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .splash {
    height: 100dvh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    color: var(--fg-muted);
    font-weight: 650;
    letter-spacing: 0.04em;
  }

  .welcome {
    height: 100dvh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }

  .card {
    max-width: 420px;
    text-align: center;
    padding: 36px 32px;
    border: 1px solid var(--border);
    border-radius: 16px;
    background: var(--bg-elevated);
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.08);
  }

  h1 {
    margin: 14px 0 10px;
    font-size: 26px;
    color: var(--accent);
    letter-spacing: 0.01em;
  }

  .tagline {
    margin: 0 0 24px;
    color: var(--fg-muted);
    line-height: 1.6;
  }

  .cta {
    padding: 11px 20px;
    border-radius: var(--radius);
    background: var(--accent);
    color: #fff;
    font-weight: 600;
  }

  .cta:hover {
    filter: brightness(1.08);
  }

  .hint {
    margin: 18px 0 0;
    font-size: 12.5px;
    color: var(--fg-muted);
  }

  .app {
    display: flex;
    flex-direction: column;
    /* dvh follows the on-screen keyboard; the viewport meta asks Android to
       resize the layout instead of panning it (app.html). */
    height: 100dvh;
  }

  .main {
    display: flex;
    flex: 1;
    min-height: 0;
    position: relative; /* anchors the note list when it works as a drawer */
  }

  .scrim {
    position: absolute;
    inset: 0;
    z-index: 65;
    background: rgba(0, 0, 0, 0.32);
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: calc(20px + env(safe-area-inset-bottom));
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-radius: var(--radius);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    color: var(--danger);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.2);
    z-index: 60;
    max-width: 80vw;
  }

  .toast.notice {
    color: var(--fg);
  }

  .toast button {
    color: var(--fg);
    padding: 3px 8px;
    border-radius: 7px;
    border: 1px solid var(--border);
  }

  /* Phone layout: the first-run card fills the width and the button is
     thumb-sized; dvh keeps both clear of the on-screen keyboard. */
  :global(:root[data-narrow]) .welcome {
    align-items: flex-start;
    padding: calc(48px + env(safe-area-inset-top)) 16px 24px;
  }

  :global(:root[data-narrow]) .card {
    width: 100%;
    padding: 28px 20px;
    border-radius: var(--radius);
  }

  :global(:root[data-narrow]) .cta {
    width: 100%;
    padding: 14px 20px;
  }

  :global(:root[data-narrow]) .toast {
    max-width: calc(100vw - 24px);
  }
</style>
