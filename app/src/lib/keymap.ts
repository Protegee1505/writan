/**
 * Global keyboard shortcuts: actions, defaults, matching, conflict checks.
 *
 * User overrides live in `settings.value.shortcuts` (action id -> binding).
 * Components dispatch through `matchesBinding`/`effectiveBinding`; the
 * settings panel records bindings with `bindingFromEvent`. No other file may
 * hardcode a global key handler (AGENTS.md).
 */
import { settings } from "./settings.svelte";

export type ActionId =
  | "command-palette"
  | "new-note"
  | "focus-search"
  | "save-note"
  | "open-settings"
  | "toggle-theme"
  | "toggle-trash"
  | "pin-note"
  | "sync-now";

export interface KeyAction {
  id: ActionId;
  label: string;
  /** Canonical binding, e.g. "Ctrl+P"; null = unbound by default. */
  defaultBinding: string | null;
  /** Extra bindings that always work but cannot be rebound. */
  aliases?: string[];
}

export const KEY_ACTIONS: KeyAction[] = [
  { id: "command-palette", label: "Command palette", defaultBinding: "Ctrl+P", aliases: ["Ctrl+K"] },
  { id: "new-note", label: "New note", defaultBinding: "Ctrl+N" },
  { id: "focus-search", label: "Find in notes", defaultBinding: "Ctrl+F" },
  { id: "save-note", label: "Save note", defaultBinding: "Ctrl+S" },
  { id: "open-settings", label: "Open settings", defaultBinding: "Ctrl+," },
  { id: "toggle-theme", label: "Toggle theme", defaultBinding: null },
  { id: "toggle-trash", label: "Toggle trash panel", defaultBinding: null },
  { id: "pin-note", label: "Pin / unpin note", defaultBinding: null },
  { id: "sync-now", label: "Sync now", defaultBinding: null },
];

/** Text-editing keys we never hijack at the app level. */
export const RESERVED_BINDINGS = new Set([
  "Ctrl+C",
  "Ctrl+V",
  "Ctrl+X",
  "Ctrl+Z",
  "Ctrl+Y",
  "Ctrl+A",
  "Ctrl+R",
  "Ctrl+W",
  "Ctrl+Shift+V",
]);

/** Reserved combos must not be bound to app actions. */
export function isReserved(binding: string | null): boolean {
  return binding !== null && RESERVED_BINDINGS.has(binding);
}

/** The binding that currently applies to an action (override or default). */
export function effectiveBinding(id: ActionId): string | null {
  const overrides = settings.value.shortcuts;
  if (Object.prototype.hasOwnProperty.call(overrides, id)) return overrides[id] ?? null;
  return KEY_ACTIONS.find((action) => action.id === id)?.defaultBinding ?? null;
}

export function isOverridden(id: ActionId): boolean {
  return Object.prototype.hasOwnProperty.call(settings.value.shortcuts, id);
}

export function actionLabel(id: ActionId): string {
  return KEY_ACTIONS.find((action) => action.id === id)?.label ?? id;
}

/** Canonical binding for a key event, or null when it has no Ctrl/Alt. */
export function bindingFromEvent(event: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (event.ctrlKey || event.metaKey) mods.push("Ctrl");
  if (event.altKey) mods.push("Alt");
  if (event.shiftKey) mods.push("Shift");
  if (mods.length === 0) return null;
  let key = event.key;
  if (key === " ") key = "Space";
  else if (key.length === 1) key = key.toUpperCase();
  return [...mods, key].join("+");
}

/** Which action (other than `id`) already uses this binding, if any. */
export function bindingConflict(id: ActionId, binding: string): ActionId | null {
  for (const action of KEY_ACTIONS) {
    if (action.id === id) continue;
    if (effectiveBinding(action.id) === binding) return action.id;
  }
  return null;
}

/**
 * True when the event triggers the action (current binding or a fixed alias).
 */
export function matchesAction(event: KeyboardEvent, id: ActionId): boolean {
  const action = KEY_ACTIONS.find((entry) => entry.id === id);
  if (!action) return false;
  const binding = bindingFromEvent(event);
  if (binding === null) return false;
  if (binding === effectiveBinding(id)) return true;
  return action.aliases?.includes(binding) ?? false;
}

/** CodeMirror key syntax for a canonical binding ("Ctrl+Shift+S" -> "Mod-Shift-s"). */
export function toCodeMirrorBinding(binding: string | null): string | null {
  if (!binding) return null;
  const parts = binding.split("+");
  const key = parts.pop();
  if (!key) return null;
  const mods = parts.map((mod) => (mod === "Ctrl" ? "Mod" : mod));
  const name = key.length === 1 ? key.toLowerCase() : key;
  return [...mods, name].join("-");
}
