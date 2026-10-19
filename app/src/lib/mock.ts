/**
 * Mock IPC backend for the visual harness (dev-only, opt-in with `?mock=1`).
 *
 * `npm --prefix app run dev` then open <http://localhost:5173/?mock=1> in any
 * Chromium browser: the real UI runs against this in-memory vault instead of
 * Tauri, so an agent can click through screens, take screenshots and read the
 * DOM without building or launching the desktop app. See
 * tools/visual/README.md for the full loop.
 *
 * It implements the commands in `lib/api.ts` plus the plugin commands the app
 * calls (dialog, opener, event). Nothing here is shipped behaviour: the import
 * is dynamic and only happens when the query parameter is present.
 *
 * Two more parameters shape the session: `&platform=android` makes the shell
 * behave like the phone build (app-private vault, no folder picker), and
 * `&welcome=1` starts with no vault open, as on a fresh install.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  CountEntry,
  Hit,
  Note,
  ReconcileStats,
  SearchFilters,
  Status,
  Summary,
  SyncConnectResult,
  SyncReport,
  SyncStatus,
  TrashEntry,
} from "./api";

const VAULT = "/home/mock/Notes";

/** `?mock=1&platform=android` makes the shell behave like the phone build. */
const params = new URLSearchParams(typeof location === "undefined" ? "" : location.search);
const platform = params.get("platform") ?? "linux";
/** Where `open_default_vault` lands; mirrors the Android build's data dir. */
const DEFAULT_VAULT = platform === "android" ? "/data/user/0/com.writan.desktop/vault" : VAULT;
/** `&welcome=1` starts with no vault open, like a fresh install. */
let vaultPath: string | null = params.get("welcome") ? null : VAULT;
/** `&share=TEXT&subject=S` simulates a share intent arriving with the launch. */
const pendingShare = { text: params.get("share"), subject: params.get("subject") };
let sharedPending = pendingShare.text !== null;

function statusOf(): Status {
  return {
    vaultPath,
    noteCount: notes.length,
    dataDir: "/home/mock/.local/share/com.writan.desktop",
    platform,
  };
}

interface MockNote extends Note {
  deletedAt?: string;
}

const seedBodies: Array<[string, string, string[], boolean]> = [
  [
    "Welcome",
    "Welcome.md",
    ["getting-started", "demo"],
    true,
  ],
  [
    "Design review",
    "Notes/Design review.md",
    ["design", "review"],
    false,
  ],
  ["日本語テスト", "Notes/日本語テスト.md", ["cjk", "demo"], false],
  ["Release checklist", "Clippings/Release checklist.md", ["demo"], false],
  ["Old idea", "Archive/Old idea.md", [], false],
];

const BODY = {
  Welcome: `# Welcome to the mock vault

This is the **browser mock backend**: the same UI, an in-memory vault.
Search for "design", open the palette with Ctrl+P, or edit this note.

- A pinned note (this one)
- Tags, folders, trash and settings all work
- Nothing is written to disk

> The real app is the source of truth; this is for looking at it.
`,
  "Design review": `# Design review

Three-pane layout: folders on the left, note list in the middle, editor on
the right. The body is long enough to scroll and to give search a snippet.

1. Keep the header bar honest.
2. Search shows the matching line.
`,
  "日本語テスト": `# 日本語テスト

このノートは CJK テキストの検索と表示を確認するためのものです。

## 中文段落

这是一个用于测试中文搜索与排版的笔记。
`,
  "Release checklist": `# Release checklist

- [x] CI green on main
- [ ] Eyeball the app on Linux and Windows
- [ ] Tag the release
`,
  "Old idea": `# Old idea

A plain note with no tags, filed under a folder. Search keyword: iguana.
`,
};

let notes: MockNote[] = seedBodies.map(([title, relPath, tags, pinned], i) => ({
  id: `mock-${i + 1}`,
  relPath,
  title,
  tags,
  pinned,
  created: `2026-10-0${i + 1}T09:00:00Z`,
  updated: `2026-10-1${i}T12:00:00Z`,
  body: BODY[title as keyof typeof BODY] ?? `# ${title}\n`,
}));
let trash: TrashEntry[] = [];
let settings: Record<string, unknown> = {};
let counter = seedBodies.length + 1;

let sync: SyncStatus = {
  enabled: false,
  unlocked: false,
  kind: "folder",
  target: null,
  serverUrl: null,
  serverEmail: null,
  vaultId: null,
  deviceId: "mock-device",
  lastSync: null,
  lastReport: null,
};

function summary(note: MockNote): Summary {
  const { body: _body, deletedAt: _deletedAt, ...rest } = note;
  return rest;
}

function sorted(): MockNote[] {
  return [...notes].sort((a, b) => {
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
    return (b.updated ?? "").localeCompare(a.updated ?? "");
  });
}

function snippet(note: MockNote, text: string): string {
  if (!text) return note.body.replace(/^#.*\n/, "").trim().slice(0, 80);
  const line = note.body
    .split("\n")
    .find((l) => l.toLowerCase().includes(text.toLowerCase()));
  return (line ?? note.body).trim().slice(0, 90);
}

function counts(names: string[]): CountEntry[] {
  const map = new Map<string, number>();
  for (const name of names) map.set(name, (map.get(name) ?? 0) + 1);
  return [...map.entries()]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function search(filters: SearchFilters): Hit[] {
  const text = (filters.text ?? "").toLowerCase();
  return sorted()
    .filter((n) => !text || `${n.title}\n${n.body}`.toLowerCase().includes(text))
    .filter((n) => !filters.tags?.length || filters.tags.every((t) => n.tags.includes(t)))
    .filter((n) => !filters.folder || n.relPath.startsWith(`${filters.folder}/`))
    .filter((n) => !filters.pinnedOnly || n.pinned)
    .slice(0, filters.limit ?? 200)
    .map((n) => ({ ...summary(n), snippet: snippet(n, filters.text ?? "") }));
}

function update(id: string, body: string, tags: string[]): Note {
  const note = notes.find((n) => n.id === id);
  if (!note) throw new Error(`mock: no note ${id}`);
  note.body = body;
  note.tags = tags;
  note.updated = new Date().toISOString();
  return { ...summary(note), body: note.body };
}

type Command = (args: Record<string, unknown>) => unknown;

const commands: Record<string, Command> = {
  get_status: statusOf,
  get_settings: () => settings,
  save_settings: (args) => {
    settings = (args.settings as Record<string, unknown>) ?? {};
    return null;
  },
  open_vault: (args) => {
    vaultPath = (args.path as string) ?? VAULT;
    return statusOf();
  },
  open_default_vault: () => {
    vaultPath = DEFAULT_VAULT;
    return statusOf();
  },
  list_notes: (args) => sorted().slice(0, (args.limit as number) ?? 200).map(summary),
  search_notes: (args) => search(args as SearchFilters),
  get_note: (args) => notes.find((n) => n.id === args.id) ?? null,
  create_note: (args) => {
    const title = ((args.title as string) ?? "Untitled").trim() || "Untitled";
    const note: MockNote = {
      id: `mock-${counter++}`,
      relPath: `${title}.md`,
      title,
      tags: [],
      pinned: false,
      created: new Date().toISOString(),
      updated: new Date().toISOString(),
      body: `# ${title}\n`,
    };
    notes.push(note);
    return summary(note);
  },
  take_shared: () => {
    if (!sharedPending || pendingShare.text === null) return null;
    sharedPending = false;
    const firstLine = pendingShare.text.split("\n").find((line) => line.trim()) ?? "";
    const title =
      (pendingShare.subject ?? firstLine).replace(/^[#>*\-\s]+/, "").trim().slice(0, 80) ||
      "Shared note";
    const note: MockNote = {
      id: `mock-${counter++}`,
      relPath: `${title}.md`,
      title,
      tags: [],
      pinned: false,
      created: new Date().toISOString(),
      updated: new Date().toISOString(),
      body: pendingShare.text,
    };
    notes.push(note);
    return note;
  },
  duplicate_note: (args) => {
    const source = notes.find((n) => n.id === args.id);
    if (!source) throw new Error(`mock: no note ${args.id}`);
    const title = `${source.title} (copy)`;
    const note: MockNote = {
      ...source,
      id: `mock-${counter++}`,
      relPath: `${title}.md`,
      title,
      created: new Date().toISOString(),
      updated: new Date().toISOString(),
    };
    notes.push(note);
    return summary(note);
  },
  save_note: (args) => update(args.id as string, args.body as string, (args.tags as string[]) ?? []),
  rename_note: (args) => {
    const note = notes.find((n) => n.id === args.id);
    if (!note) throw new Error(`mock: no note ${args.id}`);
    note.title = String(args.title);
    note.relPath = note.relPath.replace(/[^/]+\.md$/, `${args.title}.md`);
    note.updated = new Date().toISOString();
    return summary(note);
  },
  delete_note: (args): TrashEntry => {
    const index = notes.findIndex((n) => n.id === args.id);
    if (index === -1) throw new Error(`mock: no note ${args.id}`);
    const [note] = notes.splice(index, 1);
    const entry: TrashEntry = {
      id: note.id,
      title: note.title,
      tags: note.tags,
      pinned: note.pinned,
      deletedAt: new Date().toISOString(),
      originalPath: note.relPath,
      size: note.body.length,
    };
    trash.push(entry);
    return entry;
  },
  list_trash: () => trash,
  restore_note: (args) => {
    const index = trash.findIndex((e) => e.id === args.id);
    if (index === -1) throw new Error(`mock: no trashed note ${args.id}`);
    const [entry] = trash.splice(index, 1);
    const note: MockNote = {
      ...entry,
      relPath: entry.originalPath,
      created: entry.deletedAt,
      updated: entry.deletedAt,
      body: `# ${entry.title}\n`,
    };
    delete (note as { deletedAt?: string }).deletedAt;
    notes.push(note);
    return summary(note);
  },
  purge_note: (args) => {
    trash = trash.filter((e) => e.id !== args.id);
    return null;
  },
  set_pinned: (args) => {
    const note = notes.find((n) => n.id === args.id);
    if (!note) throw new Error(`mock: no note ${args.id}`);
    note.pinned = Boolean(args.pinned);
    return summary(note);
  },
  rebuild_index: (): ReconcileStats => ({
    added: 0,
    updated: 0,
    removed: 0,
    unchanged: notes.length,
    errors: 0,
  }),
  list_tags: () => counts(notes.flatMap((n) => n.tags)),
  list_folders: () => {
    const folders = notes
      .map((n) => n.relPath.split("/"))
      .filter((parts) => parts.length > 1)
      .map((parts) => parts[0]);
    return counts(folders);
  },
  save_attachment: (args) => `/home/mock/Notes/attachments/${args.fileName}`,
  import_attachment: (args) => `/home/mock/Notes/attachments/${String(args.source).split("/").pop()}`,
  sync_status: () => sync,
  sync_connect: (args): SyncConnectResult => {
    sync = {
      ...sync,
      enabled: true,
      unlocked: true,
      kind: "folder",
      target: String(args.target ?? ""),
      vaultId: "mock-vault",
      lastSync: null,
    };
    return { recoveryCode: "MOCK-RECOVERY-CODE-0000", status: sync };
  },
  sync_connect_s3: (args): SyncConnectResult => {
    sync = {
      ...sync,
      enabled: true,
      unlocked: true,
      kind: "s3",
      target: `${args.bucket} (${args.endpoint})`,
      vaultId: "mock-vault",
    };
    return { recoveryCode: "MOCK-RECOVERY-CODE-0000", status: sync };
  },
  sync_connect_server: (args): SyncConnectResult => {
    // An address with "legacy" in it plays the vault whose keyring was made
    // with an older password, so the mock loop can show the fallback prompt
    // the real connect raises (ADR-0008 §5).
    if (!args.keyringCredential && String(args.url ?? "").toLowerCase().includes("legacy")) {
      throw new Error(
        "this vault was created with a different password — enter the old sync password or the recovery code to bring it forward",
      );
    }
    sync = {
      ...sync,
      enabled: true,
      unlocked: true,
      kind: "server",
      target: String(args.url ?? ""),
      serverUrl: String(args.url ?? ""),
      serverEmail: String(args.email ?? ""),
      vaultId: "mock-vault",
    };
    return { recoveryCode: null, status: sync };
  },
  sync_join_server: (): SyncConnectResult => {
    sync = {
      ...sync,
      enabled: true,
      unlocked: true,
      kind: "server",
      target: "https://notes.example.com",
      serverUrl: "https://notes.example.com",
      serverEmail: "you@example.com",
      vaultId: "mock-vault",
    };
    return { recoveryCode: null, status: sync };
  },
  sync_change_password: (args): SyncStatus => {
    if (String(args.newPassword ?? "").length < 8) {
      throw new Error("the password must be at least 8 characters");
    }
    return sync;
  },
  sync_new_recovery_code: (): string => "MOCK-4CEF-9A20-77B1-C0DE",
  sync_unlock: (): SyncStatus => setUnlocked(true),
  sync_unlock_recovery: (): SyncStatus => setUnlocked(true),
  sync_now: (): SyncReport => {
    const report: SyncReport = {
      published: 1,
      merged: 0,
      adopted: 0,
      conflictCopies: 0,
      deleted: 0,
      unchanged: Math.max(notes.length - 1, 0),
      unreadable: 0,
    };
    sync = { ...sync, lastSync: new Date().toISOString(), lastReport: report };
    return report;
  },
  sync_lock: (): SyncStatus => setUnlocked(false),
  sync_disable: (): SyncStatus => {
    // Like the real command (app/src-tauri/src/commands.rs): turned-off sync
    // keeps kind/target/server so the setup form can prefill, and only drops
    // the session. Nulling the target here used to hide the panel-state bug.
    sync = { ...sync, enabled: false, unlocked: false, vaultId: null };
    return sync;
  },
  "plugin:dialog|open": () => VAULT,
  "plugin:dialog|save": () => `${VAULT}/export.md`,
  "plugin:opener|open_path": () => null,
  "plugin:event|listen": () => 1,
  "plugin:event|unlisten": () => null,
  "plugin:event|emit": () => null,
};

function setUnlocked(unlocked: boolean): SyncStatus {
  sync = { ...sync, unlocked };
  return sync;
}

let installed = false;

/** Install the fake IPC handler. Safe to call more than once. */
export async function installMockBackend(): Promise<void> {
  if (installed) return;
  installed = true;
  mockWindows("main");
  mockIPC((cmd, args) => {
    const handler = commands[cmd];
    if (!handler) throw new Error(`mock: unhandled command "${cmd}"`);
    return handler((args ?? {}) as Record<string, unknown>);
  });
  console.info(
    `[writan mock] in-memory vault at ${VAULT}: ?mock=1 is on, no data is written to disk`,
  );
}
