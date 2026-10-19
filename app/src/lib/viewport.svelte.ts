/**
 * Window size classes. Below the breakpoint the shell shows one pane at a
 * time: the note list fills the window when no note is open, the editor fills
 * it when one is, and the list slides over the editor as a drawer.
 *
 * The breakpoint lives here and nowhere else: `watch()` mirrors it onto
 * `<html data-narrow>`, and both components and `app.css` select on that
 * attribute, so there is a single source of truth for the phone layout.
 */
const QUERY = "(max-width: 700px)";

class ViewportStore {
  /** True when the window is phone-sized (see QUERY). */
  narrow = $state(false);

  private media: MediaQueryList | null = null;

  /** Start watching the window size and tag `<html>`; safe to call twice. */
  watch() {
    if (typeof window === "undefined" || this.media) return;
    this.media = window.matchMedia(QUERY);
    this.narrow = this.media.matches;
    this.apply();
    this.media.addEventListener("change", (event) => {
      this.narrow = event.matches;
      this.apply();
    });
  }

  private apply() {
    const root = document.documentElement;
    if (this.narrow) root.dataset.narrow = "";
    else delete root.dataset.narrow;
  }
}

export const viewport = new ViewportStore();

// Tag <html> before the first render, so a phone never paints the desktop
// shell and then reflows.
if (typeof window !== "undefined") viewport.watch();
