#!/usr/bin/env node
/**
 * Visual harness driver: plays an action script against a *running* Writan
 * window and captures screenshots, so an agent can look at the app instead
 * of asking a human to.
 *
 * The driver never launches the app; the caller does that (scripts/gui-shot.sh
 * locally, .github/workflows/visual.yml in CI) and then points the driver at
 * the window:
 *
 *   # Linux window under Xvfb (xdotool + ImageMagick)
 *   node tools/visual/drive.mjs --backend x11 --display :99 \
 *     --out /tmp/shots --actions tools/visual/actions/gtk-shell.actions
 *
 *   # Any WebView2/Chromium target launched with --remote-debugging-port=9222
 *   # (Windows CI, and `npm run dev -- --open /?mock=1` for the mock backend)
 *   node tools/visual/drive.mjs --backend cdp --cdp http://127.0.0.1:9222 \
 *     --out /tmp/shots --actions tools/visual/actions/tauri.actions
 *
 * Action script format (one command per line, `#` comments):
 *
 *   wait 800            pause, milliseconds
 *   key ctrl+f          send a chord (ctrl/alt/shift/meta + key)
 *   type hello          type literal text
 *   shot 10-search      screenshot -> <out>/10-search.png
 *   click Notes         click an element by role/text        (cdp only)
 *   dump 30-dom         write visible text -> <out>/30-dom.txt (cdp only)
 *   goto <url>          navigate                             (cdp only)
 *
 * Steps that the backend does not support are reported and skipped. By
 * default the driver keeps going after a failed step and prints a summary;
 * pass --strict to exit non-zero on the first failure.
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

// ---------------------------------------------------------------- args ----

const argv = process.argv.slice(2);
function opt(name, fallback) {
  const i = argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  const v = argv[i + 1];
  return v === undefined || v.startsWith("--") ? true : v;
}
const backend = opt("backend", "x11");
const outDir = resolve(opt("out", "/tmp/writan-shots"));
const actionsPath = opt("actions", null);
const display = opt("display", process.env.DISPLAY || ":99");
const windowName = opt("window", "Writan");
const cdpUrl = opt("cdp", "http://127.0.0.1:9222");
const readyText = opt("ready-text", null);
const strict = Boolean(opt("strict", false));
const settle = Number(opt("settle", 1200));

if (!actionsPath) {
  console.error("drive.mjs: --actions <file> is required");
  process.exit(2);
}

const failures = [];
const notes = [];
const shots = [];

function fail(step, message) {
  const line = `FAILED ${step}: ${message}`;
  failures.push(line);
  console.error(line);
  if (strict) finish(1);
}

function finish(code) {
  if (shots.length) console.log(`shots: ${shots.map((s) => s.path).join(" ")}`);
  for (const n of notes) console.log(`note: ${n}`);
  if (failures.length) console.log(`failures: ${failures.length}`);
  process.exit(code);
}

// ------------------------------------------------------------- actions ----

const actions = readFileSync(actionsPath, "utf8")
  .split("\n")
  .map((line, n) => ({ line: line.trim(), n: n + 1 }))
  .filter(({ line }) => line && !line.startsWith("#"))
  .map(({ line, n }) => {
    const space = line.indexOf(" ");
    return {
      verb: space === -1 ? line : line.slice(0, space),
      rest: space === -1 ? "" : line.slice(space + 1).trim(),
      n,
    };
  });

function translateKeys(chord) {
  // "ctrl+shift+p" -> Playwright's "Control+Shift+p"
  return chord
    .split("+")
    .map((part) => {
      const p = part.toLowerCase();
      if (p === "ctrl" || p === "control") return "Control";
      if (p === "alt") return "Alt";
      if (p === "shift") return "Shift";
      if (p === "meta" || p === "super") return "Meta";
      if (p === "enter" || p === "return") return "Enter";
      if (p === "escape" || p === "esc") return "Escape";
      if (p === "tab") return "Tab";
      if (p === "space") return "Space";
      return p;
    })
    .join("+");
}

function xdotool(args) {
  return spawnSync("xdotool", args, { encoding: "utf8", timeout: 15_000 });
}

function findWindow() {
  const res = xdotool(["search", "--onlyvisible", "--name", windowName]);
  if (res.status !== 0 || !res.stdout.trim()) return null;
  const ids = res.stdout.trim().split("\n");
  return ids[ids.length - 1];
}

/** Pick the biggest visible window matching the name (GTK opens helper 1x1 ones). */
function findMainWindow() {
  const res = xdotool(["search", "--onlyvisible", "--name", windowName]);
  if (res.status !== 0 || !res.stdout.trim()) return null;
  let best = null;
  for (const id of res.stdout.trim().split("\n")) {
    const geom = xdotool(["getwindowgeometry", "--shell", id]);
    const m = /WIDTH=(\d+)\nHEIGHT=(\d+)/.exec(geom.stdout || "");
    if (!m) continue;
    const area = Number(m[1]) * Number(m[2]);
    if (area > 1000 && (!best || area > best.area)) best = { id, area };
  }
  return best?.id ?? null;
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// --------------------------------------------------------- x11 backend ----

async function x11Setup() {
  process.env.DISPLAY = display; // every xdotool/import call below inherits it
  const probe = spawnSync("xdotool", ["getdisplaygeometry"], {
    encoding: "utf8",
    env: { ...process.env, DISPLAY: display },
  });
  if (probe.status !== 0) {
    fail("setup", `display ${display} is not reachable (${probe.stderr.trim()})`);
    finish(1);
  }
  let window = findMainWindow();
  for (let i = 0; i < 40 && !window; i++) {
    await sleep(500);
    window = findMainWindow();
  }
  if (!window) {
    fail("setup", `no visible window matching "${windowName}" after 20s`);
    finish(1);
  }
  // Without a window manager, setting input focus explicitly is what makes
  // XTEST keystrokes land in the window (Xvfb has no pointer-follow focus).
  xdotool(["windowfocus", "--sync", window]);
  xdotool(["windowraise", window]);
  return {
    run: async (action) => {
      switch (action.verb) {
        case "wait":
          await sleep(Number(action.rest));
          return;
        case "key":
        case "press": {
          const res = xdotool(["key", "--clearmodifiers", action.rest]);
          if (res.status !== 0) fail(`line ${action.n} key ${action.rest}`, res.stderr.trim());
          return;
        }
        case "type": {
          const res = xdotool(["type", "--delay", "25", "--", action.rest]);
          if (res.status !== 0) fail(`line ${action.n} type`, res.stderr.trim());
          return;
        }
        case "shot": {
          const path = join(outDir, `${action.rest || "shot"}.png`);
          const geom = xdotool(["getwindowgeometry", "--shell", window]);
          let crop = null;
          // GTK also has 1x1 helper windows; the main window is the big one.
          const m = /X=(-?\d+)\nY=(-?\d+)\nWIDTH=(\d+)\nHEIGHT=(\d+)/.exec(geom.stdout || "");
          if (m) crop = `${m[3]}x${m[4]}+${m[1]}+${m[2]}`;
          const imp = spawnSync("import", ["-display", display, "-window", "root", path], {
            encoding: "utf8",
            timeout: 30_000,
          });
          if (imp.status !== 0) {
            fail(`line ${action.n} shot`, imp.stderr.trim());
            return;
          }
          if (crop) {
            spawnSync("convert", [path, "-crop", crop, "+repage", path], { timeout: 30_000 });
          }
          shots.push({ path, step: action.n });
          return;
        }
        case "click":
        case "dump":
        case "goto":
          notes.push(`line ${action.n}: "${action.verb}" is cdp-only, skipped in x11 mode`);
          return;
        default:
          fail(`line ${action.n}`, `unknown action "${action.verb}"`);
      }
    },
  };
}

// --------------------------------------------------------- cdp backend ----

async function cdpSetup() {
  let chromium;
  try {
    ({ chromium } = await import("playwright-core"));
  } catch {
    fail("setup", "playwright-core is not installed (npm ci --prefix tools/visual)");
    finish(1);
  }
  let browser;
  for (let i = 0; ; i++) {
    try {
      browser = await chromium.connectOverCDP(cdpUrl);
      break;
    } catch (e) {
      if (i >= 5) {
        fail("setup", `cannot reach ${cdpUrl}: ${e.message}`);
        finish(1);
      }
      await sleep(1000);
    }
  }
  const contexts = browser.contexts();
  let page = null;
  for (const context of contexts) {
    for (const p of context.pages()) {
      const url = p.url();
      if (url.startsWith("devtools://") || url === "about:blank") continue;
      page = page ?? p;
    }
  }
  if (!page && contexts.length) {
    page = contexts[0].pages()[0] ?? (await contexts[0].waitForEvent("page"));
  }
  if (!page) {
    fail("setup", "connected over CDP but found no page");
    finish(1);
  }
  console.log(`cdp page: ${page.url()}`);
  if (readyText) {
    try {
      await page.waitForFunction(
        (text) => document.body && document.body.innerText.includes(text),
        readyText,
        { timeout: 60_000 },
      );
    } catch {
      fail("setup", `page never showed text "${readyText}"`);
    }
  }
  await sleep(settle);
  return {
    run: async (action) => {
      switch (action.verb) {
        case "wait":
          await sleep(Number(action.rest));
          return;
        case "key":
        case "press":
          await page.keyboard.press(translateKeys(action.rest));
          return;
        case "type":
          await page.keyboard.type(action.rest, { delay: 20 });
          return;
        case "shot": {
          const path = join(outDir, `${action.rest || "shot"}.png`);
          await page.screenshot({ path });
          shots.push({ path, step: action.n });
          return;
        }
        case "dump": {
          const path = join(outDir, `${action.rest || "dom"}.txt`);
          const text = await page.evaluate(() => {
            const parts = [`url: ${location.href}`, `title: ${document.title}`, ""];
            parts.push(`${document.body.innerText}`);
            return parts.join("\n");
          });
          writeFileSync(path, text);
          shots.push({ path, step: action.n });
          return;
        }
        case "click": {
          const byRole = page.getByRole("button", { name: action.rest });
          const target = (await byRole.count()) ? byRole.first() : page.getByText(action.rest).first();
          await target.click({ timeout: 5000 });
          return;
        }
        case "goto":
          await page.goto(action.rest);
          return;
        default:
          fail(`line ${action.n}`, `unknown action "${action.verb}"`);
      }
    },
  };
}

// ----------------------------------------------------------------- run ----

mkdirSync(outDir, { recursive: true });
console.log(`driver: backend=${backend} out=${outDir} actions=${actionsPath}`);
const driver = backend === "cdp" ? await cdpSetup() : await x11Setup();

for (const action of actions) {
  try {
    await driver.run(action);
  } catch (e) {
    fail(`line ${action.n} ${action.verb}`, e.message);
  }
}
console.log(`drive.mjs: ${shots.length} artefacts, ${failures.length} failures -> ${outDir}`);
finish(failures.length && strict ? 1 : 0);
