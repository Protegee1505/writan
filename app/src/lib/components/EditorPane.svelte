<script lang="ts">
  import { onMount } from "svelte";
  import { EditorState, Compartment } from "@codemirror/state";
  import {
    EditorView,
    keymap,
    drawSelection,
    highlightActiveLine,
    highlightSpecialChars,
    placeholder,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { livePreview } from "$lib/editor/markdown-live";
  import { store } from "$lib/store.svelte";

  let host: HTMLDivElement | undefined = $state();
  let view: EditorView | null = null;
  let lastId: string | null = null;
  let applying = false;
  let titleDraft = $state("");
  let lastTitleId: string | null = null;

  const themeComp = new Compartment();

  const baseTheme = EditorView.theme({
    "&": { height: "100%", backgroundColor: "var(--bg-elevated)", color: "var(--fg)" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", fontSize: "14.5px", lineHeight: "1.7" },
    ".cm-content": { padding: "22px 0", caretColor: "var(--accent)" },
    ".cm-line": { padding: "0 28px" },
    "&.cm-focused": { outline: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: "var(--accent-soft) !important",
    },
  });

  function currentTheme() {
    return store.theme === "dark" ? oneDark : [];
  }

  onMount(() => {
    if (!host) return;
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: store.note?.body ?? "",
        extensions: [
          history(),
          drawSelection(),
          highlightActiveLine(),
          highlightSpecialChars(),
          EditorView.lineWrapping,
          markdown(),
          livePreview(),
          placeholder("Start writing…"),
          keymap.of([
            {
              key: "Mod-s",
              preventDefault: true,
              run: () => {
                void store.saveNow();
                return true;
              },
            },
            ...defaultKeymap,
            ...historyKeymap,
            indentWithTab,
          ]),
          baseTheme,
          themeComp.of(currentTheme()),
          EditorView.updateListener.of((update) => {
            if (applying) return;
            if (update.docChanged) store.onBodyChange(update.state.doc.toString());
          }),
        ],
      }),
    });
    lastId = store.note?.id ?? null;
    return () => {
      view?.destroy();
      view = null;
    };
  });

  $effect(() => {
    const id = store.note?.id ?? null;
    if (!view || id === lastId) return;
    lastId = id;
    const body = store.note?.body ?? "";
    applying = true;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: body },
      selection: { anchor: 0 },
      scrollIntoView: false,
    });
    applying = false;
    view.focus();
  });

  $effect(() => {
    const dark = store.theme === "dark";
    if (view) view.dispatch({ effects: themeComp.reconfigure(dark ? oneDark : []) });
  });

  $effect(() => {
    const id = store.note?.id ?? null;
    if (id !== lastTitleId) {
      lastTitleId = id;
      titleDraft = store.note?.title ?? "";
    }
  });

  function commitTitle() {
    const note = store.note;
    if (!note) return;
    const clean = titleDraft.trim();
    if (!clean) {
      titleDraft = note.title;
      return;
    }
    if (clean !== note.title) void store.rename(clean);
  }

  const saveLabel = $derived(
    store.saveState === "saving"
      ? "Saving…"
      : store.saveState === "dirty"
        ? "Unsaved"
        : store.saveState === "error"
          ? "Save failed"
          : store.saveState === "saved"
            ? "Saved"
            : "",
  );
</script>

<section class="pane">
  <div class="header">
    <input
      class="titleInput"
      bind:value={titleDraft}
      onblur={commitTitle}
      onkeydown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
      }}
      placeholder="Untitled"
      spellcheck="false"
      disabled={!store.note}
    />
    <div class="meta">
      {#if store.note}
        <span class="muted">{store.wordCount} words</span>
        <span class="muted save" class:error={store.saveState === "error"}>{saveLabel}</span>
        <button
          class="pinBtn"
          class:active={store.note.pinned}
          onclick={() => store.togglePin()}
          title={store.note.pinned ? "Unpin" : "Pin"}
        >
          {store.note.pinned ? "Pinned" : "Pin"}
        </button>
        <button class="danger" onclick={() => store.removeNote()} title="Move to trash">
          Delete
        </button>
      {/if}
    </div>
  </div>

  <div class="editorBody">
    <div class="cmHost" bind:this={host}></div>
    {#if !store.note}
      <div class="overlay">
        <p>Select a note on the left — or press <kbd>Ctrl</kbd>+<kbd>N</kbd> for a new one.</p>
      </div>
    {/if}
  </div>
</section>

<style>
  .pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg-elevated);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px 10px 28px;
    border-bottom: 1px solid var(--border);
  }

  .titleInput {
    flex: 1;
    min-width: 0;
    font-size: 16px;
    font-weight: 600;
    padding: 4px 0;
  }

  .titleInput::placeholder {
    color: var(--fg-muted);
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    font-size: 12px;
  }

  .muted {
    color: var(--fg-muted);
  }

  .save.error {
    color: var(--danger);
  }

  .pinBtn,
  .danger {
    padding: 5px 10px;
    border-radius: 8px;
  }

  .pinBtn:hover,
  .danger:hover {
    background: var(--bg-hover);
  }

  .pinBtn.active {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .danger {
    color: var(--danger);
  }

  .editorBody {
    flex: 1;
    min-height: 0;
    position: relative;
  }

  .cmHost {
    position: absolute;
    inset: 0;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-muted);
    background: var(--bg-elevated);
  }

  kbd {
    font-family: var(--font-mono);
    font-size: 11.5px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    padding: 1px 5px;
    background: var(--bg);
  }
</style>
