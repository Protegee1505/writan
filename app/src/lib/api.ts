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

/** A tag or folder name with how many notes carry it. */
export interface CountEntry {
  name: string;
  count: number;
}

export interface SearchFilters {
  text?: string;
  tags?: string[];
  folder?: string | null;
  pinnedOnly?: boolean;
  limit?: number;
}

export const api = {
  status: () => invoke<Status>("get_status"),
  openVault: (path: string) => invoke<Status>("open_vault", { path }),
  list: (limit?: number) => invoke<Summary[]>("list_notes", { limit }),
  search: (filters: SearchFilters = {}) =>
    invoke<Hit[]>("search_notes", {
      text: filters.text ?? "",
      tags: filters.tags ?? [],
      folder: filters.folder ?? null,
      pinnedOnly: filters.pinnedOnly ?? false,
      limit: filters.limit,
    }),
  get: (id: string) => invoke<Note | null>("get_note", { id }),
  create: (title?: string) => invoke<Note>("create_note", { title }),
  save: (id: string, body: string, tags: string[]) =>
    invoke<Note>("save_note", { id, body, tags }),
  rename: (id: string, title: string) => invoke<Note>("rename_note", { id, title }),
  remove: (id: string) => invoke<TrashEntry>("delete_note", { id }),
  trash: () => invoke<TrashEntry[]>("list_trash"),
  restore: (id: string) => invoke<Note>("restore_note", { id }),
  purge: (id: string) => invoke<void>("purge_note", { id }),
  setPinned: (id: string, pinned: boolean) => invoke<Note>("set_pinned", { id, pinned }),
  rebuild: () => invoke<ReconcileStats>("rebuild_index"),
  listTags: () => invoke<CountEntry[]>("list_tags"),
  listFolders: () => invoke<CountEntry[]>("list_folders"),
};
