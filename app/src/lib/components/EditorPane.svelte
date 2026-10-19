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
  import { api } from "$lib/api";
  import { store } from "$lib/store.svelte";
  import { settings } from "$lib/settings.svelte";
  import { effectiveBinding, toCodeMirrorBinding } from "$lib/keymap";

  let host: HTMLDivElement | undefined = $state();
  let view: EditorView | null = null;
  let lastId: string | null = null;
  let applying = false;
  let titleDraft = $state("");
  let titleEl = $state<HTMLInputElement | undefined>();
  let lastTitleId: string | null = null;
  let tagDraft = $state("");
  let deleteArmed = $state(false);
  let deleteTimer: ReturnType<typeof setTimeout> | null = null;

  const themeComp = new Compartment();
  const wrapComp = new Compartment();
  const previewComp = new Compartment();
  const saveKeyComp = new Compartment();

  const baseTheme = EditorView.theme({
    "&": { height: "100%", backgroundColor: "var(--bg-elevated)", color: "var(--fg)" },
    ".cm-scroller": {
      fontFamily: "var(--editor-font)",
      fontSize: "var(--editor-font-size)",
      lineHeight: "var(--editor-line-height)",
    },
    ".cm-content": { padding: "22px 0", caretColor: "var(--accent)" },
    ".cm-line": { padding: "0 var(--editor-pad)" },
    "&.cm-focused": { outline: "none" },
    ".cm-activeLine": { backgroundColor: "transparent" },
    ".cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: "var(--accent-soft) !important",
    },
  });

  function currentTheme() {
    return settings.dark ? oneDark : [];
  }

  /** CodeMirror save binding follows the rebindable "Save note" action. */
  function saveKeymap(binding: string | null) {
    const key = toCodeMirrorBinding(binding);
    if (!key) return [];
    return keymap.of([
      {
        key,
        preventDefault: true,
        run: () => {
          void store.saveNow();
          return true;
        },
      },
    ]);
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
          wrapComp.of(settings.value.editor.wrap ? EditorView.lineWrapping : []),
          markdown(),
          previewComp.of(settings.value.editor.livePreview ? livePreview() : []),
          EditorView.domEventHandlers({
            paste: (event, view) => {
              const files = Array.from(event.clipboardData?.files ?? []);
              if (files.length === 0) return false;
              event.preventDefault();
              void attachFiles(files, view);
              return true;
            },
          }),
          placeholder("Start writing…"),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          saveKeyComp.of(saveKeymap(effectiveBinding("save-note"))),
          baseTheme,
          themeComp.of(currentTheme()),
          EditorView.updateListener.of((update) => {
            if (applying) return;
            if (update.docChanged) store.onBodyChange(update.state.doc.toString());
          }),
        ],
      }),
    });
    const onInsert = (e: Event) => {
      insertText((e as CustomEvent<string>).detail);
    };
    window.addEventListener("writan-insert", onInsert);
    // The note list's "Rename…" item hands focus to the title field.
    const onRename = () => {
      titleEl?.focus();
      titleEl?.select();
    };
    window.addEventListener("writan-rename", onRename);
    lastId = store.note?.id ?? null;
    return () => {
      window.removeEventListener("writan-insert", onInsert);
      window.removeEventListener("writan-rename", onRename);
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
    const dark = settings.dark;
    if (view) view.dispatch({ effects: themeComp.reconfigure(dark ? oneDark : []) });
  });

  $effect(() => {
    const wrap = settings.value.editor.wrap;
    if (view) view.dispatch({ effects: wrapComp.reconfigure(wrap ? EditorView.lineWrapping : []) });
  });

  $effect(() => {
    const on = settings.value.editor.livePreview;
    if (view) view.dispatch({ effects: previewComp.reconfigure(on ? livePreview() : []) });
  });

  $effect(() => {
    const binding = effectiveBinding("save-note");
    const extension = saveKeymap(binding);
    if (view) view.dispatch({ effects: saveKeyComp.reconfigure(extension) });
  });

  $effect(() => {
    const id = store.note?.id ?? null;
    if (id !== lastTitleId) {
      lastTitleId = id;
      titleDraft = store.note?.title ?? "";
      tagDraft = "";
      deleteArmed = false;
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

  function addTag() {
    const note = store.note;
    const clean = tagDraft.trim().replace(/^#/, "");
    tagDraft = "";
    if (!note || !clean || note.tags.includes(clean)) return;
    void store.setTags([...note.tags, clean]);
  }

  function removeTag(tag: string) {
    const note = store.note;
    if (!note) return;
    void store.setTags(note.tags.filter((t) => t !== tag));
  }

  function deleteNote() {
    if (!settings.value.general.confirmDelete) {
      void store.removeNote();
      return;
    }
    if (deleteTimer) clearTimeout(deleteTimer);
    if (!deleteArmed) {
      deleteArmed = true;
      deleteTimer = setTimeout(() => (deleteArmed = false), 3000);
      return;
    }
    deleteArmed = false;
    void store.removeNote();
  }

  async function attachFiles(files: File[], view: EditorView) {
    for (const file of files) {
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const rel = await api.saveAttachment(file.name || "pasted.png", Array.from(bytes));
        insertSnippet(view, file.name || "attachment", file.type, rel);
        store.notify(`Attached ${file.name || "image"}`);
      } catch (e) {
        store.error = String(e);
      }
    }
  }

  function insertSnippet(view: EditorView, name: string, mime: string, rel: string) {
    const isImage =
      mime.startsWith("image/") || /\.(png|jpe?g|gif|webp|svg|avif|bmp)$/i.test(name);
    const alt = name.replace(/\.[^.]+$/, "") || "image";
    const snippet = isImage ? `![${alt}](${rel})\n` : `[${name}](${rel})\n`;
    insertTextAt(view, snippet);
  }

  function insertTextAt(view: EditorView, text: string) {
    const pos = view.state.selection.main.from;
    view.dispatch({
      changes: { from: pos, insert: text },
      selection: { anchor: pos + text.length },
    });
    view.focus();
  }

  function insertText(text: string) {
    if (!view) return;
    insertTextAt(view, text);
  }

  const newNoteHint = $derived(effectiveBinding("new-note") ?? "Ctrl+N");

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
      bind:this={titleEl}
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
        {#if settings.value.view.showWordCount}
          <span class="muted">{store.wordCount} words</span>
        {/if}
        <span class="muted save" class:error={store.saveState === "error"}>{saveLabel}</span>
        <button
          class="pinBtn"
          class:active={store.note.pinned}
          onclick={() => store.togglePin()}
          title={store.note.pinned ? "Unpin" : "Pin"}
        >
          {store.note.pinned ? "Pinned" : "Pin"}
        </button>
        <button
          class="danger"
          class:armed={deleteArmed}
          onclick={deleteNote}
          title={settings.value.general.confirmDelete ? "Click again to confirm" : "Move to trash"}
        >
          {deleteArmed ? "Confirm delete" : "Delete"}
        </button>
      {/if}
    </div>
  </div>

  {#if store.note}
    <div class="tagRow">
      {#each store.note.tags as tag (tag)}
        <button class="tagChip" onclick={() => removeTag(tag)} title="Remove tag">
          #{tag}<span class="x">×</span>
        </button>
      {/each}
      <input
        class="tagInput"
        placeholder="Add tag…"
        bind:value={tagDraft}
        spellcheck="false"
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            addTag();
          }
        }}
        onblur={addTag}
      />
    </div>
  {/if}

  <div class="editorBody">
    <div class="cmHost" bind:this={host}></div>
    {#if !store.note}
      <div class="overlay">
        <p>Select a note on the left — or press <kbd>{newNoteHint}</kbd> for a new one.</p>
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
    padding: 10px 16px 10px var(--editor-pad);
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

  .danger.armed {
    background: var(--danger);
    color: #fff;
  }

  .tagRow {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 7px 16px 7px var(--editor-pad);
    border-bottom: 1px solid var(--border);
    min-height: 35px;
  }

  .tagChip {
    font-size: 11.5px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
  }

  .tagChip .x {
    margin-left: 3px;
    opacity: 0.7;
  }

  .tagInput {
    font-size: 12.5px;
    min-width: 90px;
    flex: 1;
    padding: 2px 0;
  }

  .tagInput::placeholder {
    color: var(--fg-muted);
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

  /* Phone layout: the title takes its own row above the note's controls,
     and the controls grow to finger size. */
  :global(:root[data-narrow]) .header {
    flex-wrap: wrap;
    gap: 6px 10px;
  }

  :global(:root[data-narrow]) .titleInput {
    flex: 1 1 100%;
    font-size: 17px;
    padding: 6px 0 2px;
  }

  :global(:root[data-narrow]) .meta {
    flex: 1;
  }

  :global(:root[data-narrow]) .pinBtn,
  :global(:root[data-narrow]) .danger {
    padding: 10px 13px;
  }

  :global(:root[data-narrow]) .tagRow {
    padding-bottom: 10px;
  }
</style>
