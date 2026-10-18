import { invoke } from "@tauri-apps/api/core";

export interface Status {
  vaultPath: string | null;
  noteCount: number;
  dataDir: string;
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

/** Outcome of one sync pass (how many notes each action touched). */
export interface SyncReport {
  published: number;
  merged: number;
  adopted: number;
  conflictCopies: number;
  deleted: number;
  unchanged: number;
  unreadable: number;
}

export interface SyncStatus {
  enabled: boolean;
  unlocked: boolean;
  /** "folder", "s3" or "server". */
  kind: string;
  target: string | null;
  vaultId: string | null;
  deviceId: string | null;
  lastSync: string | null;
  lastReport: SyncReport | null;
}

export interface SyncConnectResult {
  /** Shown exactly once, when this device created the keyring. */
  recoveryCode: string | null;
  status: SyncStatus;
}

/** A fresh pairing code and its QR (ADR-0007 §4). */
export interface PairingInfo {
  code: string;
  expiresAt: string;
  /** The exact text the QR carries, for devices without a camera. */
  payload: string;
  /** The QR as a standalone SVG document. */
  svg: string;
}

export const api = {
  status: () => invoke<Status>("get_status"),
  getSettings: () => invoke<Record<string, unknown>>("get_settings"),
  saveSettings: (settings: Record<string, unknown>) =>
    invoke<void>("save_settings", { settings }),
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
  emptyTrash: () => invoke<number>("empty_trash"),
  setPinned: (id: string, pinned: boolean) => invoke<Note>("set_pinned", { id, pinned }),
  rebuild: () => invoke<ReconcileStats>("rebuild_index"),
  listTags: () => invoke<CountEntry[]>("list_tags"),
  listFolders: () => invoke<CountEntry[]>("list_folders"),
  saveAttachment: (fileName: string, bytes: number[]) =>
    invoke<string>("save_attachment", { fileName, bytes }),
  importAttachment: (source: string) => invoke<string>("import_attachment", { source }),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncConnect: (target: string, password: string) =>
    invoke<SyncConnectResult>("sync_connect", { target, password }),
  syncConnectS3: (
    endpoint: string,
    bucket: string,
    region: string,
    accessKey: string,
    secretKey: string,
    password: string,
  ) =>
    invoke<SyncConnectResult>("sync_connect_s3", {
      endpoint,
      bucket,
      region,
      accessKey,
      secretKey,
      password,
    }),
  syncUnlock: (password: string) => invoke<SyncStatus>("sync_unlock", { password }),
  syncUnlockRecovery: (code: string) => invoke<SyncStatus>("sync_unlock_recovery", { code }),
  syncConnectServer: (
    url: string,
    email: string,
    accountPassword: string,
    createAccount: boolean,
    password: string,
  ) =>
    invoke<SyncConnectResult>("sync_connect_server", {
      url,
      email,
      accountPassword,
      createAccount,
      password,
    }),
  syncJoinServer: (payload: string, password: string) =>
    invoke<SyncConnectResult>("sync_join_server", { payload, password }),
  syncPairingCode: () => invoke<PairingInfo>("sync_pairing_code"),
  syncNow: () => invoke<SyncReport>("sync_now"),
  syncLock: () => invoke<SyncStatus>("sync_lock"),
  syncDisable: () => invoke<SyncStatus>("sync_disable"),
};
