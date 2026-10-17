<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api } from "$lib/api";
  import { store } from "$lib/store.svelte";

  let kind = $state<"folder" | "s3">("folder");
  let target = $state("");
  let s3Endpoint = $state("");
  let s3Bucket = $state("");
  let s3Region = $state("");
  let s3AccessKey = $state("");
  let s3SecretKey = $state("");
  let password = $state("");
  let code = $state("");
  let useRecovery = $state(false);
  let working = $state(false);
  let recoveryCode = $state<string | null>(null);
  let copied = $state(false);
  let confirmDisable = $state(false);

  const status = $derived(store.sync);
  const setupComplete = $derived(Boolean(status?.target));

  async function chooseFolder() {
    const path = await openDialog({ directory: true, title: "Choose a sync folder" });
    if (typeof path === "string") target = path;
  }

  async function connect() {
    if (working) return;
    if (kind === "folder" && !target.trim()) {
      store.error = "Choose a sync folder first.";
      return;
    }
    if (kind === "s3") {
      if (!s3Endpoint.trim()) {
        store.error = "Set the S3 endpoint first.";
        return;
      }
      if (!s3Bucket.trim()) {
        store.error = "Set the bucket first.";
        return;
      }
      if (!s3AccessKey.trim() || !s3SecretKey.trim()) {
        store.error = "Set the access key and secret first.";
        return;
      }
    }
    if (!password) {
      store.error = "Set a sync password first.";
      return;
    }
    working = true;
    store.error = null;
    try {
      const result =
        kind === "s3"
          ? await api.syncConnectS3(
              s3Endpoint.trim(),
              s3Bucket.trim(),
              s3Region.trim(),
              s3AccessKey.trim(),
              s3SecretKey.trim(),
              password,
            )
          : await api.syncConnect(target.trim(), password);
      recoveryCode = result.recoveryCode;
      password = "";
      s3SecretKey = "";
      store.sync = result.status;
      store.notify(
        result.recoveryCode
          ? "Sync enabled. Save the recovery code — it is shown only now."
          : kind === "s3"
            ? "Sync connected to the bucket."
            : "Sync connected to the shared folder.",
      );
      await store.refresh();
      void store.loadFacets();
    } catch (e) {
      store.error = String(e);
    } finally {
      working = false;
    }
  }

  async function unlock() {
    if (working) return;
    working = true;
    store.error = null;
    try {
      store.sync = useRecovery
        ? await api.syncUnlockRecovery(code)
        : await api.syncUnlock(password);
      password = "";
      code = "";
      store.notify("Sync unlocked — the key is back on this device.");
      await store.refresh();
      void store.loadFacets();
    } catch (e) {
      store.error = String(e);
    } finally {
      working = false;
    }
  }

  async function lock() {
    try {
      store.sync = await api.syncLock();
      store.notify("Sync locked. The key was forgotten on this device.");
    } catch (e) {
      store.error = String(e);
    }
  }

  async function disable() {
    if (!confirmDisable) {
      confirmDisable = true;
      setTimeout(() => (confirmDisable = false), 3000);
      return;
    }
    try {
      store.sync = await api.syncDisable();
      confirmDisable = false;
      store.notify("Sync is off for this vault. The target was left untouched.");
    } catch (e) {
      store.error = String(e);
    }
  }

  async function copyCode() {
    if (!recoveryCode) return;
    try {
      await navigator.clipboard.writeText(recoveryCode);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      store.error = "Could not copy — select the code and copy it manually.";
    }
  }

  function shortId(id: string | null | undefined): string {
    return id ? id.slice(0, 8) : "—";
  }

  function reportLine(): string {
    const report = status?.lastReport;
    if (!report) return "";
    const parts = [
      `${report.published} published`,
      `${report.merged} merged`,
      `${report.adopted} adopted`,
      `${report.conflictCopies} conflict ${report.conflictCopies === 1 ? "copy" : "copies"}`,
      `${report.deleted} deleted`,
      `${report.unchanged} unchanged`,
    ];
    if (report.unreadable) parts.push(`${report.unreadable} unreadable`);
    return parts.join(" · ");
  }
</script>

<div class="rows">
  {#if recoveryCode}
    <div class="row column boxed">
      <div class="meta">
        <span class="label">Recovery code — save it now</span>
        <span class="desc">
          The only way back in if you forget the password. It is shown here once; Writan does
          not keep a copy.
        </span>
      </div>
      <div class="control">
        <code class="recovery">{recoveryCode}</code>
        <button class="btn" onclick={() => void copyCode()}>{copied ? "Copied" : "Copy"}</button>
      </div>
    </div>
  {/if}

  {#if !setupComplete}
    <div class="row">
      <div class="meta">
        <span class="label">Where notes sync</span>
        <span class="desc">
          A folder every device can reach (a synced drive, a network share) or an S3-compatible
          bucket. Notes go in encrypted; the target never holds plaintext.
        </span>
      </div>
      <div class="control">
        <button class="btn" class:selected={kind === "folder"} onclick={() => (kind = "folder")}>
          Shared folder
        </button>
        <button class="btn" class:selected={kind === "s3"} onclick={() => (kind = "s3")}>
          S3 bucket
        </button>
      </div>
    </div>
    {#if kind === "folder"}
      <div class="row column">
        <div class="meta">
          <span class="label">Shared folder</span>
        </div>
        <div class="control wide">
          <input
            class="path"
            bind:value={target}
            placeholder="Choose a shared folder…"
            spellcheck="false"
          />
          <button class="btn" onclick={() => void chooseFolder()}>Choose folder…</button>
        </div>
      </div>
    {:else}
      <div class="row column">
        <div class="meta">
          <span class="label">Endpoint</span>
          <span class="desc">MinIO or any S3-compatible service; AWS accepts path-style too.</span>
        </div>
        <div class="control wide">
          <input
            class="path"
            bind:value={s3Endpoint}
            placeholder="https://s3.example.com"
            spellcheck="false"
          />
          <input class="path small" bind:value={s3Bucket} placeholder="bucket" spellcheck="false" />
        </div>
      </div>
      <div class="row column">
        <div class="meta">
          <span class="label">Region and credentials</span>
          <span class="desc">The secret key is kept in the OS keychain, never in the config.</span>
        </div>
        <div class="control wide">
          <input
            class="path small"
            bind:value={s3Region}
            placeholder="region (us-east-1)"
            spellcheck="false"
          />
          <input
            class="path small"
            bind:value={s3AccessKey}
            placeholder="access key"
            spellcheck="false"
          />
          <input
            class="path small"
            type="password"
            bind:value={s3SecretKey}
            placeholder="secret key"
            spellcheck="false"
          />
        </div>
      </div>
    {/if}
    <div class="row">
      <div class="meta">
        <span class="label">Sync password</span>
        <span class="desc">
          Wraps this vault's key. The first device creates the keyring and shows a recovery
          code; other devices join with the same password.
        </span>
      </div>
      <div class="control">
        <input
          type="password"
          bind:value={password}
          placeholder="Password"
          spellcheck="false"
          onkeydown={(e) => {
            if (e.key === "Enter") void connect();
          }}
        />
        <button class="btn" disabled={working} onclick={() => void connect()}>
          {working ? "Working…" : "Enable sync"}
        </button>
      </div>
    </div>
  {:else if !status?.unlocked}
    <div class="row column">
      <div class="meta">
        <span class="label">Locked</span>
        <span class="desc">
          Sync is set up to <code>{status?.target}</code> but the key is not on this device
          right now.
        </span>
      </div>
    </div>
    <div class="row">
      <div class="meta">
        <span class="label">{useRecovery ? "Recovery code" : "Password"}</span>
        {#if useRecovery}
          <button class="link" onclick={() => (useRecovery = false)}>
            Use the password instead
          </button>
        {:else}
          <button class="link" onclick={() => (useRecovery = true)}>Forgot the password?</button>
        {/if}
      </div>
      <div class="control">
        {#if useRecovery}
          <input
            bind:value={code}
            placeholder="XXXX-XXXX-…"
            spellcheck="false"
            onkeydown={(e) => {
              if (e.key === "Enter") void unlock();
            }}
          />
        {:else}
          <input
            type="password"
            bind:value={password}
            placeholder="Password"
            spellcheck="false"
            onkeydown={(e) => {
              if (e.key === "Enter") void unlock();
            }}
          />
        {/if}
        <button class="btn" disabled={working} onclick={() => void unlock()}>
          {working ? "Working…" : "Unlock"}
        </button>
      </div>
    </div>
  {:else}
    <div class="row column">
      <div class="meta">
        <span class="label">Connected</span>
        <span class="desc">
          Target <code>{status?.target}</code> · vault {shortId(status?.vaultId)} · device
          {shortId(status?.deviceId)}
        </span>
      </div>
      <div class="control">
        <button class="btn" onclick={() => void store.syncNow()}>Sync now</button>
        <button class="btn" onclick={() => void lock()}>Lock</button>
        <button class="btn danger" class:armed={confirmDisable} onclick={() => void disable()}>
          {confirmDisable ? "Click again to confirm" : "Turn off"}
        </button>
      </div>
    </div>
    <div class="row column">
      <div class="meta">
        <span class="label">Last sync</span>
        <span class="desc">{status?.lastSync ?? "Not synced yet on this device."}</span>
      </div>
      {#if reportLine()}
        <div class="control"><span class="report">{reportLine()}</span></div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .rows {
    display: flex;
    flex-direction: column;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 10px 8px;
    border-bottom: 1px solid var(--border);
  }

  .row:last-child {
    border-bottom: none;
  }

  .row.column {
    align-items: flex-start;
    flex-direction: column;
    gap: 8px;
  }

  .row.boxed {
    background: var(--accent-soft);
    border-radius: 8px;
    border-bottom: none;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .label {
    font-weight: 550;
  }

  .desc {
    font-size: 12px;
    color: var(--fg-muted);
  }

  .control {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }

  .control.wide {
    flex-wrap: wrap;
  }

  input {
    padding: 6px 10px;
    border-radius: 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--fg);
  }

  input:focus {
    border-color: var(--accent);
  }

  .path {
    width: min(320px, 40vw);
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .path.small {
    width: min(170px, 22vw);
  }

  .btn {
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--border);
    color: var(--fg);
    white-space: nowrap;
  }

  .btn:hover {
    background: var(--bg-hover);
  }

  .btn.danger {
    color: var(--danger);
  }

  .btn.selected {
    background: var(--accent-soft);
    border-color: var(--accent);
  }

  .btn.armed {
    background: var(--danger);
    color: #fff;
    border-color: var(--danger);
  }

  .link {
    font-size: 12px;
    color: var(--accent);
    padding: 2px 4px;
    border-radius: 6px;
  }

  .link:hover {
    background: var(--accent-soft);
  }

  code {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--fg-muted);
    word-break: break-all;
  }

  .recovery {
    font-size: 13px;
    color: var(--fg);
    letter-spacing: 0.06em;
  }

  .report {
    font-size: 12px;
    color: var(--fg-muted);
  }
</style>
