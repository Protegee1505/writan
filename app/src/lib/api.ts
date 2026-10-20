import { invoke } from "@tauri-apps/api/core";

export interface Status {
  vaultPath: string | null;
  noteCount: number;
  dataDir: string;
  /** The OS the app runs on: "linux", "macos", "windows", "android", "ios". */
  platform: string;
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
  /** The remembered writan-server address (or the build's baked default) and
   * account, for form prefill. */
  serverUrl: string | null;
  serverEmail: string | null;
  vaultId: string | null;
  deviceId: string | null;
  lastSync: string | null;
  lastReport: SyncReport | null;
  /** Opt-in 15-minute background sync (Android, M5 slice 2c). */
  background: boolean;
}

export interface SyncConnectResult {
  /** Shown exactly once, when this device created the keyring. */
  recoveryCode: string | null;
  status: SyncStatus;
}

/** What an export wrote. The zip is plain text — exactly what is on disk. */
export interface ExportReport {
  notes: number;
  attachments: number;
  /** Dotfiles, symlinks and OS junk the walk passed over. */
  skipped: number;
  bytes: number;
}

/** What an import added. `renamed` counts notes whose id was already taken. */
export interface ImportReport extends ExportReport {
  renamed: number;
}

/**
 * The older credential a retry carries after a connect failed with "this vault
 * was created with a different password" (ADR-0008 §5): the password the
 * vault's keyring was made with, or its recovery code.
 */
export interface KeyringFallback {
  purpose: "password" | "recovery";
  credential: string;
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
  openDefaultVault: () => invoke<Status>("open_default_vault"),
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
  takeShared: () => invoke<Note | null>("take_shared"),
  duplicate: (id: string) => invoke<Note>("duplicate_note", { id }),
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
  exportBackup: (path: string) => invoke<ExportReport>("export_backup", { path }),
  importBackup: (path: string) => invoke<ImportReport>("import_backup", { path }),
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
    createAccount: boolean,
    password: string,
    fallback?: KeyringFallback,
  ) =>
    invoke<SyncConnectResult>("sync_connect_server", {
      url,
      email,
      createAccount,
      password,
      keyringPurpose: fallback?.purpose,
      keyringCredential: fallback?.credential,
    }),
  syncJoinServer: (payload: string, password: string, fallback?: KeyringFallback) =>
    invoke<SyncConnectResult>("sync_join_server", {
      payload,
      password,
      keyringPurpose: fallback?.purpose,
      keyringCredential: fallback?.credential,
    }),
  syncPairingCode: () => invoke<PairingInfo>("sync_pairing_code"),
  syncChangePassword: (oldPassword: string, newPassword: string) =>
    invoke<SyncStatus>("sync_change_password", { oldPassword, newPassword }),
  syncNewRecoveryCode: () => invoke<string>("sync_new_recovery_code"),
  syncNow: () => invoke<SyncReport>("sync_now"),
  syncSetBackground: (enabled: boolean) =>
    invoke<SyncStatus>("sync_set_background", { enabled }),
  syncLock: () => invoke<SyncStatus>("sync_lock"),
  syncDisable: () => invoke<SyncStatus>("sync_disable"),
};
