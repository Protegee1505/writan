# Visual harness — look at the app yourself

Three loops for seeing what the UI actually does, fastest first. They exist so
an agent can debug the app without asking a human to run it and describe it.

## 1. Mock loop (seconds) — the Svelte UI in Chromium

```sh
npm --prefix app run dev
# then open  http://localhost:5173/?mock=1
# or one command:
scripts/mock-shot.sh --serve
```

`?mock=1` installs `app/src/lib/mock.ts`: the real frontend against an
in-memory vault instead of Tauri. Nothing loads without the flag. Chromium is
the same engine as WebView2, so this is also a good preview of the Windows
app. In OpenChamber, the browser panel can drive the same URL
(`browser.snapshot` / `browser.capture`).

Not covered here: watcher events, real files on disk, E2EE, huge vaults.

## 2. Linux app loop (about a minute) — the GTK shell

```sh
scripts/gui-shot.sh --bin target/release/writan            # dark
scripts/gui-shot.sh --bin target/release/writan --theme light --out /tmp/light
```

Runs the real binary under Xvfb with the fixture vault, plays
`tools/visual/actions/gtk-shell.actions`, and writes cropped PNGs to
`/tmp/writan-shots`. The app never touches your real app data (temp
`XDG_CONFIG_HOME`/`XDG_DATA_HOME`) or the fixture vault (a copy).

## 3. CI `visual.yml` — real Windows and Linux builds

```sh
gh workflow run visual.yml -R Protegee1505/writan
gh run watch
gh run download <id> -n writan-windows-shots
```

- `windows-ui` builds the Tauri app debug binary **with the frontend embedded**
  and **with a debugging port in the window config** (injected into
  `tauri.conf.json` for that build only): WebView2 ignores
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` once wry sets the property through
  the API, and `tauri/build.rs` sends a `cargo build` without the
  `custom-protocol` feature at `devUrl` instead of the built frontend. It seeds
  `%APPDATA%\com.writan.desktop\config.json` with the fixture vault, launches
  the app, waits for `http://127.0.0.1:9222/json/version`, then drives it over
  CDP — screenshots and a DOM text dump, no interactive session required.
- `linux-ui` runs loop 2 on a runner that has the fonts (incl. CJK installed).

## Action scripts

`tools/visual/drive.mjs` drives an **already running** window (the script or
the workflow launches the app). One command per line, `#` comments:

```
wait 800          # milliseconds
key ctrl+f        # ctrl/alt/shift/meta chords
type hello        # literal text
shot 10-search    # -> <out>/10-search.png
click Notes       # click by role/text            (cdp backend only)
dump 30-dom       # visible text -> <out>/30-dom.txt (cdp backend only)
goto <url>        # navigate                      (cdp backend only)
```

Two backends: `--backend x11` (xdotool + ImageMagick, Linux/Xvfb) and
`--backend cdp` (playwright-core; anything launched with
`--remote-debugging-port=9222` — WebView2, Chromium, the mock loop). Steps a
backend does not support are reported and skipped. Exit code is 0 with a
failure summary on stdout; add `--strict` to fail on the first failed step.

`dump` is the cheap way for an agent to read the UI as text instead of pixels.

## Fixtures

`tools/visual/fixtures/vault/` — a small vault: a pinned note, tags, two
folders, a task list, CJK (CJK search + font fallback), a long body for
scrolling. Both loops copy it to a temp directory first; nothing writes into
the repo. Add a note here when a screen needs content to look real.
