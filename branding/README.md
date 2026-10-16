# Writan branding

- `writan-icon.svg` — app icon master (rounded square + W, fixed brand colours).
- `writan-mark.svg` — the W alone, for one-colour use.

The in-app logo is `app/src/lib/components/Logo.svelte`, which renders the same
geometry with the live theme accent.

To regenerate the OS bundle icons (Windows, macOS, iOS, Android):

```sh
npm --prefix app run tauri -- icon ../branding/writan-icon.svg
```

The Tauri CLI rasterizes the SVG directly; `writan-icon.png` (1024×1024) is a
raster export kept for tools that cannot read SVG. Icons under
`app/src-tauri/icons/` were generated from this master on 2026-10-16.
