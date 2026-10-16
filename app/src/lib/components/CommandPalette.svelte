<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api } from "$lib/api";
  import { store } from "$lib/store.svelte";

  let { open = $bindable(), onclose }: { open: boolean; onclose: () => void } = $props();

  let query = $state("");
  let active = $state(0);
  let inputEl = $state<HTMLInputElement | undefined>();

  interface Item {
    id: string;
    label: string;
    hint: string;
    run: () => void | Promise<void>;
  }

  const items = $derived.by(() => {
    const all: Item[] = [];
    if (store.status.vaultPath) {
      all.push({ id: "new", label: "New note", hint: "Ctrl+N", run: () => store.createNote() });
      all.push({ id: "save", label: "Save now", hint: "Ctrl+S", run: () => store.saveNow() });
      all.push({
        id: "attach",
        label: "Attach file…",
        hint: "Attachment",
        run: async () => {
          const path = await openDialog({ multiple: false, directory: false, title: "Attach file" });
          if (typeof path !== "string") return;
          try {
            const rel = await api.importAttachment(path);
            const name = path.split(/[\\/]/).pop() ?? "file";
            window.dispatchEvent(
              new CustomEvent("writan-insert", { detail: `[${name}](${rel})\n` }),
            );
            store.notify(`Attached ${name}`);
          } catch (e) {
            store.error = String(e);
          }
        },
      });
    }
    all.push({
      id: "theme",
      label: store.theme === "dark" ? "Switch to light theme" : "Switch to dark theme",
      hint: "Theme",
      run: () => store.toggleTheme(),
    });
    if (store.status.vaultPath) {
      all.push({
        id: "trash",
        label: store.trashOpen ? "Close trash" : "Open trash",
        hint: "Trash",
        run: () => store.toggleTrash(),
      });
      all.push({
        id: "rebuild",
        label: "Rebuild search index",
        hint: "Maintenance",
        run: () => store.rebuildIndex(),
      });
      if (store.note) {
        all.push({
          id: "pin",
          label: store.note.pinned ? "Unpin this note" : "Pin this note",
          hint: "Pin",
          run: () => store.togglePin(),
        });
      }
      for (const note of store.visible) {
        all.push({
          id: `note-${note.id}`,
          label: note.title || "Untitled",
          hint: "Open note",
          run: () => store.select(note.id),
        });
      }
    }
    const q = query.trim().toLowerCase();
    if (!q) return all;
    return all.filter((item) => item.label.toLowerCase().includes(q));
  });

  $effect(() => {
    if (open) {
      query = "";
      active = 0;
      requestAnimationFrame(() => inputEl?.focus());
    }
  });

  function runItem(item: Item) {
    open = false;
    void item.run();
  }

  function onKey(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      onclose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      active = Math.min(active + 1, items.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      active = Math.max(active - 1, 0);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[active];
      if (item) runItem(item);
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div class="overlay">
    <button class="backdrop" aria-label="Close command palette" onclick={onclose}></button>
    <div class="palette">
      <input
        bind:this={inputEl}
        bind:value={query}
        placeholder="Type a command or note name…"
        spellcheck="false"
        oninput={() => (active = 0)}
      />
      <div class="list">
        {#each items as item, i (item.id)}
          <button
            class="row"
            class:active={i === active}
            onmouseenter={() => (active = i)}
            onclick={() => runItem(item)}
          >
            <span class="label">{item.label}</span>
            <span class="hint">{item.hint}</span>
          </button>
        {/each}
        {#if items.length === 0}
          <div class="empty">No matches</div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 70;
  }

  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.32);
    border: none;
    border-radius: 0;
    cursor: default;
  }

  .palette {
    position: relative;
    width: min(560px, 90vw);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .palette input {
    padding: 13px 16px;
    font-size: 14.5px;
    border-bottom: 1px solid var(--border);
    background: transparent;
  }

  .palette input::placeholder {
    color: var(--fg-muted);
  }

  .list {
    max-height: 320px;
    overflow-y: auto;
    padding: 6px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border-radius: 8px;
  }

  .row.active {
    background: var(--accent-soft);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    flex: none;
    font-size: 11.5px;
    color: var(--fg-muted);
  }

  .empty {
    padding: 18px 12px;
    text-align: center;
    color: var(--fg-muted);
    font-size: 13px;
  }
</style>
