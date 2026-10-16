<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { store } from "$lib/store.svelte";
  import TopBar from "$lib/components/TopBar.svelte";
  import NoteList from "$lib/components/NoteList.svelte";
  import EditorPane from "$lib/components/EditorPane.svelte";
  import TrashPanel from "$lib/components/TrashPanel.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";

  let unlisten: (() => void) | null = null;
  let paletteOpen = $state(false);

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

  function onKey(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    if (e.key === "p" || e.key === "k") {
      e.preventDefault();
      paletteOpen = true;
    } else if (e.key === "n" && store.status.vaultPath) {
      e.preventDefault();
      void store.createNote();
    } else if (e.key === "f") {
      e.preventDefault();
      (document.getElementById("writan-search") as HTMLInputElement | null)?.focus();
    } else if (e.key === "s") {
      void store.saveNow();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if !store.ready}
  <div class="splash">Writan</div>
{:else if store.status.vaultPath === null}
  <main class="welcome">
    <div class="card">
      <h1>Writan</h1>
      <p class="tagline">A fast, local-first notebook. Your notes are plain Markdown files in a folder you own.</p>
      <button class="cta" onclick={() => store.chooseVault()}>Choose notes folder</button>
      <p class="hint">Nothing leaves your machine.</p>
    </div>
  </main>
{:else}
  <div class="app">
    <TopBar />
    <div class="main">
      <NoteList />
      <EditorPane />
    </div>
    {#if store.trashOpen}
      <TrashPanel />
    {/if}
    <CommandPalette bind:open={paletteOpen} onclose={() => (paletteOpen = false)} />
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
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    font-weight: 650;
    letter-spacing: 0.04em;
  }

  .welcome {
    height: 100vh;
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
    margin: 0 0 10px;
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
    height: 100vh;
  }

  .main {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 20px;
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
</style>
