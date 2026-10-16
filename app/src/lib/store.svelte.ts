import { api, type Hit, type Note, type Status, type Summary, type TrashEntry } from "./api";

export type SaveState = "idle" | "dirty" | "saving" | "saved" | "error";

class AppStore {
  ready = $state(false);
  status = $state<Status>({ vaultPath: null, noteCount: 0 });
  notes = $state<Summary[]>([]);
  hits = $state<Hit[] | null>(null);
  query = $state("");
  selectedId = $state<string | null>(null);
  note = $state<Note | null>(null);
  saveState = $state<SaveState>("idle");
  error = $state<string | null>(null);
  theme = $state<"light" | "dark">("light");
  trash = $state<TrashEntry[]>([]);
  trashOpen = $state(false);

  // Not reactive on purpose.
  saveTimer: ReturnType<typeof setTimeout> | null = null;
  searchTimer: ReturnType<typeof setTimeout> | null = null;

  get visible(): (Summary | Hit)[] {
    return this.hits ?? this.notes;
  }

  get wordCount(): number {
    const body = this.note?.body ?? "";
    return body.split(/\s+/).filter(Boolean).length;
  }

  async init() {
    const stored = localStorage.getItem("writan-theme");
    const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    this.theme = stored === "light" || stored === "dark" ? stored : prefersDark ? "dark" : "light";
    this.applyTheme();
    try {
      this.status = await api.status();
      if (this.status.vaultPath) {
        await this.refresh();
        const first = this.notes[0];
        if (first) await this.select(first.id);
      }
    } catch (e) {
      this.error = String(e);
    } finally {
      this.ready = true;
    }
  }

  applyTheme() {
    if (typeof document !== "undefined") {
      document.documentElement.dataset.theme = this.theme;
    }
    localStorage.setItem("writan-theme", this.theme);
  }

  toggleTheme() {
    this.theme = this.theme === "dark" ? "light" : "dark";
    this.applyTheme();
  }

  async chooseVault() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const path = await open({ directory: true, title: "Choose your notes folder" });
    if (typeof path !== "string") return;
    try {
      this.status = await api.openVault(path);
      this.note = null;
      this.selectedId = null;
      this.query = "";
      this.hits = null;
      await this.refresh();
    } catch (e) {
      this.error = String(e);
    }
  }

  async refresh() {
    try {
      if (this.query.trim()) {
        this.hits = await api.search(this.query.trim());
      } else {
        this.hits = null;
        this.notes = await api.list();
      }
    } catch (e) {
      this.error = String(e);
    }
  }

  setQuery(q: string) {
    this.query = q;
    if (this.searchTimer) clearTimeout(this.searchTimer);
    this.searchTimer = setTimeout(() => void this.refresh(), 250);
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
    this.saveTimer = setTimeout(() => void this.saveNow(), 700);
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
      const saved = await api.save(note.id, body);
      if (this.note?.id === saved.id) {
        this.note.updated = saved.updated;
        if (this.note.body !== body) {
          this.saveState = "dirty";
          this.saveTimer = setTimeout(() => void this.saveNow(), 700);
        } else {
          this.saveState = "saved";
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
      await this.refresh();
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

  async onExternalChange(rels: string[]) {
    await this.refresh();
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
