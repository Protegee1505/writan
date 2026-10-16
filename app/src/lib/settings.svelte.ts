/**
 * User preferences: schema, defaults, apply pipeline, persistence.
 *
 * Rust stores the object verbatim in `<app-data>/config.json` under `settings`
 * (see docs/adr/0003-app-preferences.md). This registry owns the schema, so a
 * new feature only ever adds its options here (AGENTS.md: "Settings-first").
 */
import { api } from "./api";

export type ThemeMode = "light" | "dark" | "system";
export type SectionId =
  | "appearance"
  | "editor"
  | "view"
  | "shortcuts"
  | "search"
  | "general"
  | "advanced";

export interface AccentPreset {
  id: string;
  label: string;
  light: string;
  dark: string;
}

/** Accent colors for light and dark themes. Keep in sync with app.css defaults. */
export const ACCENTS: AccentPreset[] = [
  { id: "indigo", label: "Indigo", light: "#5b6ee0", dark: "#93a0ff" },
  { id: "violet", label: "Violet", light: "#7c5ce0", dark: "#b6a4ff" },
  { id: "teal", label: "Teal", light: "#0f9d8f", dark: "#5fd7c6" },
  { id: "green", label: "Green", light: "#3f9b4f", dark: "#7fd48d" },
  { id: "amber", label: "Amber", light: "#b9811a", dark: "#e9b95c" },
  { id: "rose", label: "Rose", light: "#c04b73", dark: "#f09ab5" },
];

export interface MonoFont {
  id: string;
  label: string;
  stack: string;
}

export const MONO_FONTS: MonoFont[] = [
  {
    id: "system",
    label: "System Mono",
    stack: 'ui-monospace, "Cascadia Code", "JetBrains Mono", Consolas, monospace',
  },
  { id: "cascadia", label: "Cascadia Code", stack: '"Cascadia Code", Consolas, monospace' },
  { id: "jetbrains", label: "JetBrains Mono", stack: '"JetBrains Mono", Consolas, monospace' },
  { id: "consolas", label: "Consolas", stack: 'Consolas, "Courier New", monospace' },
  { id: "courier", label: "Courier New", stack: '"Courier New", monospace' },
];

export interface Settings {
  appearance: {
    theme: ThemeMode;
    accent: string;
    /** Webview zoom factor, 1 = 100%. */
    uiScale: number;
  };
  editor: {
    fontFamily: string;
    fontSize: number;
    lineHeight: number;
    wrap: boolean;
    livePreview: boolean;
    autosaveMs: number;
  };
  view: {
    listDensity: "comfortable" | "compact";
    showWordCount: boolean;
    dateFormat: "relative" | "absolute";
  };
  search: {
    live: boolean;
    debounceMs: number;
  };
  general: {
    confirmDelete: boolean;
    reopenLastNote: boolean;
  };
  /** action id -> binding override; `null` = explicitly unbound. */
  shortcuts: Record<string, string | null>;
}

export const DEFAULT_SETTINGS: Settings = {
  appearance: { theme: "system", accent: "indigo", uiScale: 1 },
  editor: {
    fontFamily: "system",
    fontSize: 14.5,
    lineHeight: 1.7,
    wrap: true,
    livePreview: true,
    autosaveMs: 700,
  },
  view: { listDensity: "comfortable", showWordCount: true, dateFormat: "relative" },
  search: { live: true, debounceMs: 150 },
  general: { confirmDelete: true, reopenLastNote: true },
  shortcuts: {},
};

/** App-managed values that ride in the same file but are not preferences. */
interface SessionState {
  lastNoteId: string | null;
}

export const SECTIONS: { id: SectionId; label: string }[] = [
  { id: "appearance", label: "Appearance" },
  { id: "editor", label: "Editor" },
  { id: "view", label: "View" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "search", label: "Search" },
  { id: "general", label: "General" },
  { id: "advanced", label: "Advanced" },
];

export type ControlSpec =
  | { kind: "select"; choices: { value: string | number; label: string }[] }
  | { kind: "toggle" }
  | { kind: "slider"; min: number; max: number; step: number; unit?: string }
  | { kind: "accent" };

export interface OptionSpec {
  /** Dotted path into Settings, e.g. "editor.fontSize". */
  path: string;
  section: SectionId;
  label: string;
  description?: string;
  keywords?: string[];
  control: ControlSpec;
}

export const OPTIONS: OptionSpec[] = [
  {
    path: "appearance.theme",
    section: "appearance",
    label: "Theme",
    description: "System follows your OS setting",
    keywords: ["dark", "light", "mode", "night"],
    control: {
      kind: "select",
      choices: [
        { value: "light", label: "Light" },
        { value: "dark", label: "Dark" },
        { value: "system", label: "System" },
      ],
    },
  },
  {
    path: "appearance.accent",
    section: "appearance",
    label: "Accent color",
    keywords: ["color", "highlight"],
    control: { kind: "accent" },
  },
  {
    path: "appearance.uiScale",
    section: "appearance",
    label: "Interface scale",
    description: "Zooms the whole window",
    keywords: ["zoom", "size", "bigger", "smaller"],
    control: {
      kind: "select",
      choices: [
        { value: 0.9, label: "90%" },
        { value: 1, label: "100%" },
        { value: 1.1, label: "110%" },
        { value: 1.25, label: "125%" },
      ],
    },
  },
  {
    path: "editor.fontFamily",
    section: "editor",
    label: "Font family",
    keywords: ["typeface", "mono", "font"],
    control: {
      kind: "select",
      choices: MONO_FONTS.map((f) => ({ value: f.id, label: f.label })),
    },
  },
  {
    path: "editor.fontSize",
    section: "editor",
    label: "Font size",
    keywords: ["text size", "bigger", "smaller"],
    control: { kind: "slider", min: 11, max: 24, step: 0.5, unit: "px" },
  },
  {
    path: "editor.lineHeight",
    section: "editor",
    label: "Line height",
    keywords: ["spacing", "leading"],
    control: { kind: "slider", min: 1.3, max: 2.2, step: 0.05 },
  },
  {
    path: "editor.wrap",
    section: "editor",
    label: "Wrap long lines",
    keywords: ["word wrap"],
    control: { kind: "toggle" },
  },
  {
    path: "editor.livePreview",
    section: "editor",
    label: "Live preview",
    description: "Style Markdown while you type",
    keywords: ["markdown", "preview"],
    control: { kind: "toggle" },
  },
  {
    path: "editor.autosaveMs",
    section: "editor",
    label: "Autosave delay",
    description: "Pause after typing before saving",
    keywords: ["save", "debounce"],
    control: { kind: "slider", min: 200, max: 2000, step: 100, unit: "ms" },
  },
  {
    path: "view.listDensity",
    section: "view",
    label: "Note list density",
    keywords: ["compact", "spacious", "sidebar"],
    control: {
      kind: "select",
      choices: [
        { value: "comfortable", label: "Comfortable" },
        { value: "compact", label: "Compact" },
      ],
    },
  },
  {
    path: "view.showWordCount",
    section: "view",
    label: "Show word count",
    keywords: ["stats", "words"],
    control: { kind: "toggle" },
  },
  {
    path: "view.dateFormat",
    section: "view",
    label: "Date format",
    keywords: ["time", "relative", "absolute"],
    control: {
      kind: "select",
      choices: [
        { value: "relative", label: "Relative (2h ago)" },
        { value: "absolute", label: "Absolute (Oct 16, 14:00)" },
      ],
    },
  },
  {
    path: "search.live",
    section: "search",
    label: "Search while typing",
    description: "Results update as you type — no Enter needed",
    keywords: ["dynamic", "instant", "find"],
    control: { kind: "toggle" },
  },
  {
    path: "search.debounceMs",
    section: "search",
    label: "Debounce",
    description: "How long typing pauses before searching",
    keywords: ["delay", "typing"],
    control: { kind: "slider", min: 50, max: 500, step: 25, unit: "ms" },
  },
  {
    path: "general.confirmDelete",
    section: "general",
    label: "Confirm before deleting",
    description: "Asks before a note moves to the trash",
    keywords: ["trash", "delete", "confirm"],
    control: { kind: "toggle" },
  },
  {
    path: "general.reopenLastNote",
    section: "general",
    label: "Reopen last note on launch",
    keywords: ["startup", "session", "restore"],
    control: { kind: "toggle" },
  },
];

export function getPath(root: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((obj, key) => {
    if (obj && typeof obj === "object") return (obj as Record<string, unknown>)[key];
    return undefined;
  }, root);
}

function setPath(root: Record<string, unknown>, path: string, value: unknown): void {
  const parts = path.split(".");
  let obj: Record<string, unknown> = root;
  for (const part of parts.slice(0, -1)) {
    const next = obj[part];
    if (!next || typeof next !== "object") return;
    obj = next as Record<string, unknown>;
  }
  const last = parts[parts.length - 1];
  if (last !== undefined) obj[last] = value;
}

/** Deep-merge stored values over defaults; unknown keys are dropped. */
export function mergeSettings(defaults: Settings, raw: unknown): Settings {
  const out = structuredClone(defaults);
  if (!raw || typeof raw !== "object") return out;
  const stored = raw as Record<string, unknown>;
  for (const section of ["appearance", "editor", "view", "search", "general"] as const) {
    const values = stored[section];
    if (!values || typeof values !== "object") continue;
    for (const [key, value] of Object.entries(values)) {
      const current = (out[section] as Record<string, unknown>)[key];
      if (current !== undefined && typeof value === typeof current) {
        (out[section] as Record<string, unknown>)[key] = value;
      }
    }
  }
  const keys = stored.shortcuts;
  if (keys && typeof keys === "object") {
    const map: Record<string, string | null> = {};
    for (const [id, binding] of Object.entries(keys)) {
      if (binding === null || typeof binding === "string") map[id] = binding;
    }
    out.shortcuts = map;
  }
  if (!ACCENTS.some((a) => a.id === out.appearance.accent)) out.appearance.accent = ACCENTS[0]!.id;
  const font = MONO_FONTS.find((f) => f.id === out.editor.fontFamily);
  if (!font) out.editor.fontFamily = MONO_FONTS[0]!.id;
  return out;
}

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return hex;
  const n = parseInt(m[1]!, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

class SettingsStore {
  value = $state<Settings>(structuredClone(DEFAULT_SETTINGS));
  loaded = $state(false);

  private session = $state<SessionState>({ lastNoteId: null });
  private systemDark = $state(false);
  private media: MediaQueryList | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private appliedScale: number | null = null;

  /** True when the effective theme is dark (explicit or via the OS). */
  get dark(): boolean {
    const mode = this.value.appearance.theme;
    return mode === "dark" || (mode === "system" && this.systemDark);
  }

  get accent(): AccentPreset {
    return ACCENTS.find((a) => a.id === this.value.appearance.accent) ?? ACCENTS[0]!;
  }

  get monoStack(): string {
    return MONO_FONTS.find((f) => f.id === this.value.editor.fontFamily)?.stack ?? MONO_FONTS[0]!.stack;
  }

  get lastNoteId(): string | null {
    return this.session.lastNoteId;
  }

  async init() {
    if (typeof window !== "undefined" && !this.media) {
      this.media = window.matchMedia("(prefers-color-scheme: dark)");
      this.systemDark = this.media.matches;
      this.media.addEventListener("change", (event) => {
        this.systemDark = event.matches;
        this.apply();
      });
    }
    let raw: Record<string, unknown> = {};
    try {
      raw = await api.getSettings();
      this.value = mergeSettings(DEFAULT_SETTINGS, raw);
    } catch {
      // Preferences are not critical; defaults are fine.
    }
    // One-time migration of the legacy localStorage theme key.
    const legacy = typeof localStorage === "undefined" ? null : localStorage.getItem("writan-theme");
    const storedTheme = getPath(raw, "appearance.theme");
    if (!storedTheme && (legacy === "light" || legacy === "dark")) {
      this.value.appearance.theme = legacy;
      this.scheduleSave();
    }
    if (legacy) localStorage.removeItem("writan-theme");
    const session = getPath(raw, "session");
    if (session && typeof session === "object") {
      const id = (session as Record<string, unknown>).lastNoteId;
      this.session.lastNoteId = typeof id === "string" ? id : null;
    }
    this.loaded = true;
    this.apply();
  }

  /** Change one option, live-apply it, persist. */
  set(path: string, value: unknown) {
    setPath(this.value as unknown as Record<string, unknown>, path, value);
    this.apply();
    this.scheduleSave();
  }

  /** Drop an override so the default takes over again (e.g. shortcuts). */
  unset(path: string) {
    const parts = path.split(".");
    let obj: Record<string, unknown> = this.value as unknown as Record<string, unknown>;
    for (const part of parts.slice(0, -1)) {
      const next = obj[part];
      if (!next || typeof next !== "object") return;
      obj = next as Record<string, unknown>;
    }
    const last = parts[parts.length - 1];
    if (last) delete obj[last];
    this.apply();
    this.scheduleSave();
  }

  resetAll() {
    this.value = structuredClone(DEFAULT_SETTINGS);
    this.apply();
    void this.save();
  }

  setLastNote(id: string | null) {
    if (this.session.lastNoteId === id) return;
    this.session.lastNoteId = id;
    this.scheduleSave();
  }

  /** Apply theme, accent, editor and scale to the DOM. Idempotent. */
  apply() {
    if (typeof document === "undefined") return;
    const root = document.documentElement;
    root.dataset.theme = this.dark ? "dark" : "light";
    const accent = this.accent;
    const color = this.dark ? accent.dark : accent.light;
    root.style.setProperty("--accent", color);
    root.style.setProperty("--accent-soft", hexToRgba(color, this.dark ? 0.24 : 0.16));
    root.style.setProperty("--editor-font", this.monoStack);
    root.style.setProperty("--editor-font-size", `${this.value.editor.fontSize}px`);
    root.style.setProperty("--editor-line-height", String(this.value.editor.lineHeight));
    void this.applyScale();
  }

  private async applyScale() {
    const scale = this.value.appearance.uiScale;
    if (this.appliedScale === scale) return;
    this.appliedScale = scale;
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      const webview = getCurrentWebview() as unknown as {
        setZoom?: (factor: number) => Promise<void>;
      };
      if (typeof webview.setZoom !== "function") throw new Error("no zoom api");
      await webview.setZoom(scale);
    } catch {
      // Non-Tauri context or no zoom API: fall back to CSS zoom.
      if (typeof document !== "undefined") {
        document.documentElement.style.setProperty("zoom", String(scale));
      }
    }
  }

  private scheduleSave() {
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void this.save(), 400);
  }

  /** Persist immediately (used e.g. when the settings panel closes). */
  flush() {
    if (this.saveTimer) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
      void this.save();
    }
  }

  private async save() {
    this.saveTimer = null;
    const payload: Record<string, unknown> = {
      ...$state.snapshot(this.value),
      session: { ...$state.snapshot(this.session) },
    };
    try {
      await api.saveSettings(payload);
    } catch (e) {
      console.error("failed to save settings", e);
    }
  }
}

export const settings = new SettingsStore();

/** Options whose label/keywords match a palette query, best first. */
export function searchSettings(query: string, limit = 6): OptionSpec[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const scored: { spec: OptionSpec; score: number }[] = [];
  for (const spec of OPTIONS) {
    const haystack = [spec.label, spec.description ?? "", ...(spec.keywords ?? [])]
      .join(" ")
      .toLowerCase();
    if (spec.label.toLowerCase().startsWith(q)) scored.push({ spec, score: 2 });
    else if (haystack.includes(q)) scored.push({ spec, score: 1 });
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.slice(0, limit).map((entry) => entry.spec);
}
