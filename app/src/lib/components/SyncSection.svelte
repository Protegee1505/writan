<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api, type KeyringFallback, type PairingInfo } from "$lib/api";
  import { store } from "$lib/store.svelte";

  // No folder target on Android: there is no folder picker there (ADR-0009
  // §2), so the picker opens on the server form instead (ADR-0010 §4).
  let kind = $state<"folder" | "s3" | "server">(store.android ? "server" : "folder");
  $effect(() => {
    if (store.android && kind === "folder") kind = "server";
  });
  let target = $state("");
  let s3Endpoint = $state("");
  let s3Bucket = $state("");
  let s3Region = $state("");
  let s3AccessKey = $state("");
  let s3SecretKey = $state("");
  let serverUrl = $state("");
  let serverEmail = $state("");
  let createAccount = $state(false);
  let joining = $state(false);
  let pairText = $state("");
  let pairing = $state<PairingInfo | null>(null);
  let pairCopied = $state(false);
  let password = $state("");
  // The vault's keyring was wrapped with an older password: after that
  // connect error the setup form asks for it (or its recovery code) once
  // (ADR-0008 §5). `fallback` holds what was typed.
  let needsFallback = $state(false);
  let fallback = $state("");
  let fallbackRecovery = $state(false);
  let code = $state("");
  let useRecovery = $state(false);
  let working = $state(false);
  let recoveryCode = $state<string | null>(null);
  let copied = $state(false);
  let confirmDisable = $state(false);
  // The connected panel's Change password row.
  let changeOld = $state("");
  let changeNew = $state("");

  const status = $derived(store.sync);
  // Sync counts as set up only while it is enabled: a turned-off target stays
  // in the config just to prefill the setup form below (GTK parity: setup =
  // !enabled). Keying off `target` alone stranded a turned-off vault on the
  // Locked panel with no way back to the method picker.
  const setupComplete = $derived(Boolean(status?.enabled));

  // Coming back to the server form (say, after Turn off) starts from the
  // remembered address and account; the token stays in the keychain and the
  // password is never stored.
  let prefilled = $state(false);
  $effect(() => {
    if (prefilled) return;
    const url = status?.serverUrl;
    const email = status?.serverEmail;
    if (!url && !email) return;
    if (!serverUrl) serverUrl = url ?? "";
    if (!serverEmail) serverEmail = email ?? "";
    prefilled = true;
  });

  async function chooseFolder() {
    const path = await openDialog({ directory: true, title: "Choose a sync folder" });
    if (typeof path === "string") target = path;
  }

  /** The credential the fallback prompt holds, in the IPC shape. */
  function fallbackCredential(): KeyringFallback {
    return {
      purpose: fallbackRecovery ? "recovery" : "password",
      credential: fallback.trim(),
    };
  }

  /**
   * Connect (or re-connect with an older credential, ADR-0008 §5). One
   * password does everything: the server derives a verifier from it, the
   * keyring wraps the vault key with it.
   */
  async function connect(older?: KeyringFallback) {
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
    if (kind === "server") {
      if (!serverUrl.trim().startsWith("http")) {
        store.error = "Set the server address first (http:// or https://).";
        return;
      }
      if (joining) {
        if (!pairText.trim()) {
          store.error = "Paste the pairing text from the other device first.";
          return;
        }
      } else if (!serverEmail.trim().includes("@")) {
        store.error = "Enter the account email first.";
        return;
      }
    }
    if (!password) {
      store.error = "Set a password first.";
      return;
    }
    if (password.length < 8) {
      store.error = "The password must be at least 8 characters.";
      return;
    }
    working = true;
    store.error = null;
    try {
      const result =
        kind === "server"
          ? joining
            ? await api.syncJoinServer(pairText.trim(), password, older)
            : await api.syncConnectServer(
                serverUrl.trim(),
                serverEmail.trim(),
                createAccount,
                password,
                older,
              )
          : kind === "s3"
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
      pairText = "";
      fallback = "";
      fallbackRecovery = false;
      needsFallback = false;
      store.sync = result.status;
      store.notify(
        result.recoveryCode
          ? "Sync enabled. Save the recovery code — it is shown only now."
          : kind === "server"
            ? joining
              ? "Joined the server with the pairing code."
              : "Sync connected to the server."
            : kind === "s3"
              ? "Sync connected to the bucket."
              : "Sync connected to the shared folder.",
      );
      await store.refresh();
      void store.loadFacets();
    } catch (e) {
      const message = String(e);
      // This is the one error that a second credential can fix, so the form
      // asks for it instead of just reporting (ADR-0008 §5).
      if (message.includes("different password")) {
        needsFallback = true;
      }
      store.error = message;
    } finally {
      working = false;
    }
  }

  /** Change the one password: keyring first, then the account (ADR-0008 §6). */
  async function changePassword() {
    if (working) return;
    if (!changeOld) {
      store.error = "Enter the current password first.";
      return;
    }
    if (changeNew.length < 8) {
      store.error = "The new password must be at least 8 characters.";
      return;
    }
    working = true;
    store.error = null;
    try {
      store.sync = await api.syncChangePassword(changeOld, changeNew);
      changeOld = "";
      changeNew = "";
      store.notify("Password changed. Other devices must sign in again.");
      await store.refresh();
    } catch (e) {
      store.error = String(e);
    } finally {
      working = false;
    }
  }

  /** Mint a fresh recovery code; it opens a new keyring generation only for it. */
  async function newRecoveryCode() {
    if (working) return;
    working = true;
    store.error = null;
    try {
      recoveryCode = await api.syncNewRecoveryCode();
      copied = false;
      store.notify("New recovery code — it is shown only now.");
    } catch (e) {
      store.error = String(e);
    } finally {
      working = false;
    }
  }

  /** Mint a one-time pairing code and show its QR (ADR-0007 §4). */
  async function showPairing() {
    if (working) return;
    working = true;
    store.error = null;
    try {
      pairing = await api.syncPairingCode();
      pairCopied = false;
    } catch (e) {
      store.error = String(e);
    } finally {
      working = false;
    }
  }

  async function copyPairing() {
    if (!pairing) return;
    try {
      await navigator.clipboard.writeText(pairing.code);
      pairCopied = true;
      setTimeout(() => (pairCopied = false), 1500);
    } catch {
      store.error = "Could not copy — select the code and copy it manually.";
    }
  }

  function when(iso: string): string {
    const date = new Date(iso);
    return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
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
          {#if store.android}
            An S3-compatible bucket or a Writan server. Notes go in encrypted; the target never
            holds plaintext.
          {:else}
            A folder every device can reach (a synced drive, a network share) or an S3-compatible
            bucket. Notes go in encrypted; the target never holds plaintext.
          {/if}
        </span>
      </div>
      <div class="control">
        {#if !store.android}
          <button class="btn" class:selected={kind === "folder"} onclick={() => (kind = "folder")}>
            Shared folder
          </button>
        {/if}
        <button class="btn" class:selected={kind === "s3"} onclick={() => (kind = "s3")}>
          S3 bucket
        </button>
        <button class="btn" class:selected={kind === "server"} onclick={() => (kind = "server")}>
          Writan server
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
    {:else if kind === "s3"}
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
    {:else}
      <div class="row column">
        <div class="meta">
          <span class="label">Server</span>
          <span class="desc">
            Your own writan-server. Notes travel through it encrypted end to end; the server
            only ever holds ciphertext.
          </span>
        </div>
        <div class="control wide">
          <input
            class="path"
            bind:value={serverUrl}
            placeholder="https://notes.example.com"
            spellcheck="false"
          />
        </div>
      </div>
      {#if joining}
        <div class="row column">
          <div class="meta">
            <span class="label">Pairing text</span>
            <span class="desc">
              Paste the text shown under the QR on a device that is already connected.
            </span>
          </div>
          <div class="control wide">
            <input
              class="path"
              bind:value={pairText}
              placeholder="Pairing text from the other device"
              spellcheck="false"
            />
            <button class="link" onclick={() => (joining = false)}>
              Sign in with the email instead
            </button>
          </div>
        </div>
      {:else}
        <div class="row column">
          <div class="meta">
            <span class="label">Account</span>
            <span class="desc">
              The email this server knows. The password below never leaves this device: the
              server only holds a verifier derived from it.
            </span>
          </div>
          <div class="control wide">
            <input
              class="path"
              bind:value={serverEmail}
              placeholder="you@example.com"
              spellcheck="false"
            />
          </div>
        </div>
        <div class="row">
          <div class="meta">
            <span class="label">First device here?</span>
            <span class="desc">Create the account when the server does not know this email yet.</span>
          </div>
          <div class="control">
            <label class="check">
              <input type="checkbox" bind:checked={createAccount} />
              Create the account
            </label>
            <button class="link" onclick={() => (joining = true)}>Have a pairing code?</button>
          </div>
        </div>
      {/if}
    {/if}
    <div class="row">
      <div class="meta">
        <span class="label">Password</span>
        <span class="desc">
          One password for the whole vault: it signs this device in on a server and wraps the
          vault key. The first device creates the keyring and shows a recovery code; every
          other device joins with the same password.
        </span>
      </div>
      <div class="control">
        <input
          type="password"
          bind:value={password}
          placeholder="At least 8 characters"
          spellcheck="false"
          onkeydown={(e) => {
            if (e.key === "Enter") void connect();
          }}
        />
        <button class="btn" disabled={working} onclick={() => void connect()}>
          {working ? "Working…" : kind === "server" && joining ? "Join" : "Enable sync"}
        </button>
      </div>
    </div>
    {#if needsFallback}
      <div class="row column boxed">
        <div class="meta">
          <span class="label">{fallbackRecovery ? "Older recovery code" : "Older password"}</span>
          <span class="desc">
            This vault's key is wrapped with a different password. Enter the one it was made
            with (or its recovery code) to bring the vault forward to the password above.
          </span>
          {#if fallbackRecovery}
            <button class="link" onclick={() => (fallbackRecovery = false)}>
              Use the old password instead
            </button>
          {:else}
            <button class="link" onclick={() => (fallbackRecovery = true)}>
              Have the recovery code?
            </button>
          {/if}
        </div>
        <div class="control wide">
          <input
            type={fallbackRecovery ? "text" : "password"}
            bind:value={fallback}
            placeholder={fallbackRecovery ? "XXXX-XXXX-…" : "Old password"}
            spellcheck="false"
            onkeydown={(e) => {
              if (e.key === "Enter") void connect(fallbackCredential());
            }}
          />
          <button class="btn" disabled={working} onclick={() => void connect(fallbackCredential())}>
            {working ? "Working…" : "Bring it forward"}
          </button>
        </div>
      </div>
    {/if}
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
    {#if status?.kind === "server"}
      <div class="row column">
        <div class="meta">
          <span class="label">Pair a device</span>
          <span class="desc">
            Show a one-time QR for a new device. It carries blob access only — the new device
            still needs the same password or its recovery code.
          </span>
        </div>
        <div class="control">
          <button class="btn" disabled={working} onclick={() => void showPairing()}>
            {pairing ? "New code" : "Show pairing code"}
          </button>
        </div>
      </div>
      {#if pairing}
        <div class="row column boxed">
          <div class="qr" aria-label="Pairing QR code">{@html pairing.svg}</div>
          <div class="control">
            <code class="recovery">{pairing.code}</code>
            <button class="btn" onclick={() => void copyPairing()}>
              {pairCopied ? "Copied" : "Copy code"}
            </button>
          </div>
          <span class="desc">
            Expires {when(pairing.expiresAt)}. No camera on the new device? Paste the QR text
            when it asks for a pairing code.
          </span>
        </div>
      {/if}
    {/if}
    <div class="row column">
      <div class="meta">
        <span class="label">Change password</span>
        <span class="desc">
          Rewraps this vault's key for the new password
          {status?.kind === "server" ? ", then replaces the account's verifier on the server" : ""}
          — the vault key itself never changes. Other devices must sign in again.
        </span>
      </div>
      <div class="control wide">
        <input
          type="password"
          bind:value={changeOld}
          placeholder="Current password"
          spellcheck="false"
        />
        <input
          type="password"
          bind:value={changeNew}
          placeholder="New password"
          spellcheck="false"
        />
        <button class="btn" disabled={working} onclick={() => void changePassword()}>
          {working ? "Working…" : "Change"}
        </button>
      </div>
    </div>
    <div class="row">
      <div class="meta">
        <span class="label">New recovery code</span>
        <span class="desc">
          Mint a fresh code if the old one leaked — it is shown once, and the old code keeps
          opening older versions of the key.
        </span>
      </div>
      <div class="control">
        <button class="btn" disabled={working} onclick={() => void newRecoveryCode()}>
          New code
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

  .qr {
    background: #fff;
    padding: 8px;
    border-radius: 8px;
    line-height: 0;
  }

  .qr :global(svg) {
    width: 168px;
    height: 168px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-muted);
  }

  input[type="checkbox"] {
    accent-color: var(--accent);
  }
</style>
