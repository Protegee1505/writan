import { api, type CountEntry, type Hit, type Note, type Status, type Summary, type SyncReport, type SyncStatus, type TrashEntry } from "./api";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { settings } from "./settings.svelte";

export type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

class AppStore {
  ready = $state(false);
  status = $state<Status>({ vaultPath: null, noteCount: 0, dataDir: "" });
  notes = $state<Summary[]>([]);
  hits = $state<Hit[] | null>(null);
  query = $state("");
  selectedId = $state<string | null>(null);
  note = $state<Note | null>(null);
  saveState = $state<SaveState>("idle");
  error = $state<string | null>(null);
  notice = $state<string | null>(null);
  searching = $state(false);
  trash = $state<TrashEntry[]>([]);
  trashOpen = $state(false);
  tags = $state<CountEntry[]>([]);
  folders = $state<CountEntry[]>([]);
  tagFilter = $state<string | null>(null);
  folderFilter = $state<string | null>(null);
  sync = $state<SyncStatus | null>(null);

  // Not reactive on purpose.
  saveTimer: ReturnType<typeof setTimeout> | null = null;
  searchTimer: ReturnType<typeof setTimeout> | null = null;
  noticeTimer: ReturnType<typeof setTimeout> | null = null;
  searchSeq = 0;

  get visible(): (Summary | Hit)[] {
    return this.hits ?? this.notes;
  }

  get wordCount(): number {
    const body = this.note?.body ?? "";
    return body.split(/\s+/).filter(Boolean).length;
  }

  get filtered(): boolean {
    return this.tagFilter !== null || this.folderFilter !== null;
  }

  get theme(): "light" | "dark" {
    return settings.dark ? "dark" : "light";
  }

  async init() {
    await settings.init();
    try {
      this.status = await api.status();
      if (this.status.vaultPath) {
        await this.refresh();
        await this.loadFacets();
        void this.refreshSync();
        await this.reopenOrSelectFirst();
      }
    } catch (e) {
      this.error = String(e);
    } finally {
      this.ready = true;
    }
  }

  private async reopenOrSelectFirst() {
    const last = settings.value.general.reopenLastNote ? settings.lastNoteId : null;
    if (last) {
      await this.select(last);
      if (this.note) return;
    }
    const first = this.notes[0];
    if (first) await this.select(first.id);
  }

  toggleTheme() {
    settings.set("appearance.theme", settings.dark ? "light" : "dark");
  }

  async chooseVault() {
    const path = await openDialog({ directory: true, title: "Choose your notes folder" });
    if (typeof path !== "string") return;
    try {
      this.status = await api.openVault(path);
      this.note = null;
      this.selectedId = null;
      this.query = "";
      this.hits = null;
      this.tagFilter = null;
      this.folderFilter = null;
      await this.refresh();
      await this.loadFacets();
      void this.refreshSync();
    } catch (e) {
      this.error = String(e);
    }
  }

  async refresh() {
    const seq = ++this.searchSeq;
    const filtering = Boolean(this.query.trim() || this.tagFilter || this.folderFilter);
    if (filtering) this.searching = true;
    try {
      if (filtering) {
        const hits = await api.search({
          text: this.query,
          tags: this.tagFilter ? [this.tagFilter] : [],
          folder: this.folderFilter,
          limit: 500,
        });
        if (seq !== this.searchSeq) return;
        this.hits = hits;
      } else {
        const notes = await api.list();
        if (seq !== this.searchSeq) return;
        this.hits = null;
        this.notes = notes;
      }
    } catch (e) {
      if (seq === this.searchSeq) this.error = String(e);
    } finally {
      if (seq === this.searchSeq) this.searching = false;
    }
  }

  async loadFacets() {
    try {
      this.tags = await api.listTags();
      this.folders = await api.listFolders();
    } catch (e) {
      this.error = String(e);
    }
  }

  notify(message: string) {
    this.notice = message;
    if (this.noticeTimer) clearTimeout(this.noticeTimer);
    this.noticeTimer = setTimeout(() => (this.notice = null), 2600);
  }

  async rebuildIndex() {
    try {
      const stats = await api.rebuild();
      this.notify(
        `Index rebuilt: ${stats.added} added, ${stats.updated} updated, ${stats.removed} removed.`,
      );
      await this.refresh();
      void this.loadFacets();
    } catch (e) {
      this.error = String(e);
    }
  }

  /** Refresh the sync status shown in settings and the command palette. */
  async refreshSync() {
    try {
      this.sync = await api.syncStatus();
    } catch {
      this.sync = null;
    }
  }

  private syncSummary(report: SyncReport): string {
    const parts: string[] = [];
    if (report.published) parts.push(`${report.published} published`);
    if (report.merged) parts.push(`${report.merged} merged`);
    if (report.adopted) parts.push(`${report.adopted} adopted`);
    if (report.deleted) parts.push(`${report.deleted} deleted`);
    if (report.conflictCopies) parts.push(`${report.conflictCopies} conflict copies`);
    return parts.length
      ? `Sync finished: ${parts.join(", ")}.`
      : "Sync finished: everything is already up to date.";
  }

  async syncNow() {
    try {
      const report = await api.syncNow();
      this.notify(this.syncSummary(report));
      await this.refresh();
      void this.loadFacets();
      await this.refreshSync();
    } catch (e) {
      this.error = String(e);
    }
  }

  toggleTag(tag: string) {
    this.tagFilter = this.tagFilter === tag ? null : tag;
    void this.refresh();
  }

  toggleFolder(folder: string) {
    this.folderFilter = this.folderFilter === folder ? null : folder;
    void this.refresh();
  }

  async clearFilters() {
    this.tagFilter = null;
    this.folderFilter = null;
    await this.refresh();
  }

  setQuery(q: string) {
    this.query = q;
    if (this.searchTimer) {
      clearTimeout(this.searchTimer);
      this.searchTimer = null;
    }
    if (!settings.value.search.live) return;
    this.searchTimer = setTimeout(() => void this.refresh(), settings.value.search.debounceMs);
  }

  /** Run the search immediately (Enter in the search field, or clear). */
  commitQuery() {
    if (this.searchTimer) {
      clearTimeout(this.searchTimer);
      this.searchTimer = null;
    }
    void this.refresh();
  }

  clearQuery() {
    if (!this.query) return;
    this.query = "";
    this.commitQuery();
  }

  async select(id: string) {
    if (this.selectedId === id) return;
    await this.flushSave();
    this.selectedId = id;
    this.saveState = "idle";
    try {
      const note = await api.get(id);
      if (note) {
        this.note = note;
        settings.setLastNote(id);
      } else {
        this.selectedId = null;
        await this.refresh();
      }
    } catch (e) {
      this.error = String(e);
    }
  }

  async createNote() {
    try {
      const note = await api.create();
      this.query = "";
      this.hits = null;
      await this.refresh();
      void this.loadFacets();
      await this.select(note.id);
    } catch (e) {
      this.error = String(e);
    }
  }

  onBodyChange(body: string) {
    if (!this.note) return;
    this.note.body = body;
    this.saveState = "dirty";
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void this.saveNow(), settings.value.editor.autosaveMs);
  }

  async setTags(tags: string[]) {
    const note = this.note;
    if (!note) return;
    note.tags = tags;
    this.saveState = "dirty";
    await this.saveNow();
    void this.loadFacets();
  }

  async flushSave() {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    if (this.saveState === "dirty") await this.saveNow();
  }

  async saveNow() {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    const note = this.note;
    if (!note || this.saveState === "saving") return;
    if (this.saveState !== "dirty" && this.saveState !== "error") return;
    const body = note.body;
    this.saveState = "saving";
    try {
      const saved = await api.save(note.id, body, note.tags);
      if (this.note?.id === saved.id) {
        this.note.updated = saved.updated;
        if (this.note.body !== body) {
          this.saveState = "dirty";
          this.saveTimer = setTimeout(
            () => void this.saveNow(),
            settings.value.editor.autosaveMs,
          );
        } else {
          this.saveState = "saved";
          void this.loadFacets();
        }
      } else {
        this.saveState = "idle";
      }
    } catch (e) {
      this.saveState = "error";
      this.error = String(e);
    }
  }

  async rename(title: string) {
    const note = this.note;
    const clean = title.trim();
    if (!note || !clean || clean === note.title) return;
    try {
      const renamed = await api.rename(note.id, clean);
      if (this.note?.id === renamed.id) {
        this.note.title = renamed.title;
        this.note.relPath = renamed.relPath;
      }
      await this.refresh();
    } catch (e) {
      this.error = String(e);
    }
  }

  async togglePin() {
    const note = this.note;
    if (!note) return;
    try {
      const updated = await api.setPinned(note.id, !note.pinned);
      if (this.note?.id === updated.id) this.note.pinned = updated.pinned;
      await this.refresh();
    } catch (e) {
      this.error = String(e);
    }
  }

  async removeNote() {
    const note = this.note;
    if (!note) return;
    try {
      await api.remove(note.id);
      this.note = null;
      this.selectedId = null;
      settings.setLastNote(null);
      await this.refresh();
      void this.loadFacets();
      if (this.trashOpen) await this.loadTrash();
    } catch (e) {
      this.error = String(e);
    }
  }

  async toggleTrash() {
    this.trashOpen = !this.trashOpen;
    if (this.trashOpen) await this.loadTrash();
  }

  async loadTrash() {
    try {
      this.trash = await api.trash();
    } catch (e) {
      this.error = String(e);
    }
  }

  async restore(trashId: string) {
    try {
      await api.restore(trashId);
      await this.loadTrash();
      await this.refresh();
      void this.loadFacets();
    } catch (e) {
      this.error = String(e);
    }
  }

  async purge(trashId: string) {
    try {
      await api.purge(trashId);
      await this.loadTrash();
    } catch (e) {
      this.error = String(e);
    }
  }

  async emptyTrash() {
    try {
      const removed = await api.emptyTrash();
      await this.loadTrash();
      if (removed > 0)
        this.notify(`Removed ${removed} ${removed === 1 ? "note" : "notes"} from the trash`);
    } catch (e) {
      this.error = String(e);
    }
  }

  async onExternalChange(rels: string[]) {
    await this.refresh();
    void this.loadFacets();
    const note = this.note;
    if (!note || !rels.includes(note.relPath)) return;
    if (this.saveState === "dirty" || this.saveState === "saving") return;
    try {
      const fresh = await api.get(note.id);
      if (fresh && this.note?.id === fresh.id) {
        this.note = fresh;
        this.saveState = "idle";
      }
    } catch {
      // The note may have been removed; the refresh above already handled the list.
    }
  }
}

export const store = new AppStore();
