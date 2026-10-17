// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
export const ssr = false;

/**
 * `?mock=1` runs the real UI against an in-memory backend instead of Tauri,
 * so the page can be opened, clicked and screenshotted in any Chromium
 * browser (tools/visual/README.md). No mock code loads without the flag.
 */
export async function load() {
  if (typeof window !== "undefined" && new URLSearchParams(window.location.search).has("mock")) {
    const { installMockBackend } = await import("$lib/mock");
    await installMockBackend();
  }
}
