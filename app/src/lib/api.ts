import { invoke } from "@tauri-apps/api/core";

export interface Status {
  vaultPath: string | null;
  noteCount: number;
}

export interface Summary {
  id: string;
  relPath: string;
  title: string;
  tags: string[];
  pinned: boolean;
  created: string | null;
  updated: string | null;
}

export interface Hit {
  id: string;
  relPath: string;
  title: string;
  tags: string[];
  pinned: boolean;
  snippet: string;
}

export interface Note extends Summary {
  body: string;
}

export interface TrashEntry {
  id: string;
  title: string;
  tags: string[];
  pinned: boolean;
  deletedAt: string;
  originalPath: string;
  size: number;
}

export interface ReconcileStats {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  errors: number;
}

export const api = {
  status: () => invoke<Status>("get_status"),
  openVault: (path: string) => invoke<Status>("open_vault", { path }),
  list: (limit?: number) => invoke<Summary[]>("list_notes", { limit }),
  search: (text: string, pinnedOnly = false, limit?: number) =>
    invoke<Hit[]>("search_notes", { text, pinnedOnly, limit }),
  get: (id: string) => invoke<Note | null>("get_note", { id }),
  create: (title?: string) => invoke<Note>("create_note", { title }),
  save: (id: string, body: string) => invoke<Note>("save_note", { id, body }),
  rename: (id: string, title: string) => invoke<Note>("rename_note", { id, title }),
  remove: (id: string) => invoke<TrashEntry>("delete_note", { id }),
  trash: () => invoke<TrashEntry[]>("list_trash"),
  restore: (id: string) => invoke<Note>("restore_note", { id }),
  purge: (id: string) => invoke<void>("purge_note", { id }),
  setPinned: (id: string, pinned: boolean) => invoke<Note>("set_pinned", { id, pinned }),
  rebuild: () => invoke<ReconcileStats>("rebuild_index"),
};
