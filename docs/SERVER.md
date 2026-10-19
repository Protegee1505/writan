# Self-hosting the Writan sync server

`writan-server` is the optional third sync target (after a shared folder and an
S3-compatible bucket). It is a *dumb* ciphertext store: it keeps the encrypted
blobs of your vaults, hands devices their tokens and streams change events. It
never sees a note, a title, a vault key or your password — the vault key stays
on your devices, and the password reaches the server only as a verifier derived
from it (ADR-0008). The decisions behind it are ADR-0007 and ADR-0008
(`docs/adr/`).

You do not need a server: folder and S3 sync work without one. Pick one when you
want your own always-on target, for example so phones can sync without a cloud
bucket.

## Deploy in 5 minutes (Ubuntu 22.04+ / Debian 12+, x86_64)

On a fresh VM — Oracle Cloud Always Free, Hetzner, any VPS — as root:

```sh
# a free name such as writan-notes.duckdns.org pointing at this host's IP
curl -fsSL https://github.com/Protegee1505/writan/releases/latest/download/writan-server-install.sh \
  | sudo bash -s -- --domain writan-notes.duckdns.org --registration closed
```

The installer downloads the pinned `writan-server` release asset, checks it
against the release's `SHA256SUMS.txt`, installs it as
`/usr/local/bin/writan-server`, creates the `writan` user,
`/var/lib/writan-server` and `/etc/writan-server.env`, writes a systemd unit,
points [Caddy](https://caddyserver.com/docs/install) at `127.0.0.1:8787` (Caddy
gets the HTTPS certificate on its own) and prints the address plus `/health`.
Re-run it any time to upgrade the binary in place; the data directory is never
touched.

Flags: `--domain HOST` (HTTPS through Caddy), `--local` (plain HTTP on every
interface), `--registration open|closed`, `--port N`, `--version vX.Y.Z`
(`--help` prints the same). Everything below assumes the default layout; the
service reads `/etc/writan-server.env` and logs to `journalctl -u
writan-server`.

Health: `curl https://writan-notes.duckdns.org/health` →
`{"status":"ok","version":"0.2.0","schema":2}`.

### Docker instead

```sh
git clone https://github.com/Protegee1505/writan
cd writan
docker compose up -d --build
curl http://127.0.0.1:8787/health
```

A tagged release also publishes an amd64 image:

```sh
docker run -d --name writan-server --restart unless-stopped \
  -p 127.0.0.1:8787:8787 -v writan-data:/data \
  ghcr.io/protegee1505/writan-server:latest
```

On an ARM host (for example Oracle's free Ampere VMs) there is no release
binary yet: use `docker compose up -d --build`, which compiles from source.

## Connecting the app

In Writan: **Settings → Sync**, choose **Writan server**, enter the address,
your email and **one password** (at least 8 characters), and leave **Create the
account** on the first time. That password both signs the device in and wraps
the vault key; the server stores only a verifier, never the password itself.

A second device signs in with the same email and password and every note is
there. **Pair a device** (first device) plus **Have a pairing code?** (new one)
is the shortcut when typing is awkward: the one-time code (10 minutes) grants
blob access only — the joining device still needs the password or the recovery
code.

The connected panel has **Change password** (the current one must be right;
other devices then sign in again) and **New recovery code** (shown once; the
first code is displayed when the keyring is created).

## Configuration

Everything is environment variables; there is no config file.

| Variable | Default | Meaning |
| --- | --- | --- |
| `WRITAN_BIND` | `0.0.0.0` | Address to listen on; a reverse proxy setup uses `127.0.0.1`. |
| `WRITAN_PORT` | `8787` | Listen port. |
| `WRITAN_DATA_DIR` | `writan-data` | Where `writan-server.sqlite` lives. |
| `WRITAN_REGISTRATION` | `open` | `closed` refuses new accounts. |
| `WRITAN_MAX_BLOB_KIB` | `4096` | Largest single blob; bigger uploads get 413. |
| `WRITAN_VAULT_QUOTA_MIB` | `512` | Per-vault quota; `0` = unlimited; over quota gets 507. |
| `WRITAN_PAIRING_TTL_SECS` | `600` | Lifetime of a pairing code. |

A personal app build can bake its own address so the sync form arrives filled
in: `WRITAN_DEFAULT_SERVER=https://writan-notes.duckdns.org npm --prefix app run
tauri build` (the same variable works for the GTK shell: `cargo build -p
writan-linux` with it set). Public release builds compile no default, and no
credential is ever baked in.

## TLS

The server speaks plain HTTP and expects a reverse proxy to terminate TLS; the
installer's `--domain` writes exactly this into `/etc/caddy/Caddyfile`:

```
writan-notes.duckdns.org {
    reverse_proxy 127.0.0.1:8787
}
```

With `WRITAN_BIND=127.0.0.1` the port is not reachable from the internet at
all. Do not expose it directly — without TLS, tokens and encrypted blobs travel
in the clear.

## Accounts and admin

- Create the first account from the app (leave **Create the account** on).
  Afterwards `WRITAN_REGISTRATION=closed` keeps anyone else out.
- **Change password** lives in the app (Settings → Sync, while unlocked).
- **Forgot the password** (there is no email; reset from the host):

  ```sh
  docker exec -it writan-server writan-server reset-password you@example.com
  # non-interactively:
  docker exec -e WRITAN_NEW_PASSWORD='new password' writan-server \
    writan-server reset-password you@example.com
  # systemd install: WRITAN_DATA_DIR=/var/lib/writan-server \
  #   writan-server reset-password you@example.com
  ```

  This sets a new password for the account and revokes every device token of
  it; devices sign in again. Notes already on disk stay readable — a device
  that has the vault key keeps syncing after signing in.
- A database from before v0.2 (schema 1) migrates on first open; its account
  keeps its devices but has no password material, so sign-in answers 403 with
  "this account predates this server version" until you run `reset-password`
  once for that account.
- Five failed sign-ins lock an account for 60 s, doubling up to 15 minutes.

## Backups and moving hosts

The whole server state is one SQLite file:

```sh
# systemd install
systemctl stop writan-server
cp /var/lib/writan-server/writan-server.sqlite /backup/
systemctl start writan-server

# docker
docker stop writan-server
docker run --rm -v writan-data:/data -v "$PWD":/backup debian:bookworm-slim \
  cp /data/writan-server.sqlite /backup/writan-server.sqlite
docker start writan-server
```

Restore by putting the file back (fresh host, same layout) and starting the
server. Devices keep working; nothing on the clients changes.

## Upgrades

Re-run the installer (`--version vX.Y.Z` to pin one), or for docker
`docker compose build --pull && docker compose up -d`. The store carries its
schema version; older stores migrate on open, newer ones are refused, so keep
the binary at least as new as the file and take a backup before switching
versions.

## Limits and housekeeping

- Blobs are immutable and are never deleted (no garbage collection in v1): a
  deleted note is a small tombstone, so vaults grow slowly and the quota is the
  guard rail. Raise `WRITAN_VAULT_QUOTA_MIB` if a big vault hits it.
- Everything is scoped to one account: one vault per account, and a vault id
  belonging to another account answers 404.
- `GET /health` answers `{"status":"ok","version":…,"schema":…}`; logs go to
  `journalctl -u writan-server` (or the container's stdout).

## Small hosts and free tiers

The server idles at a few tens of megabytes and needs no cache or queue, so a
1 vCPU / 1 GB VM is plenty, and the release binary needs no Docker.

- **Oracle Cloud Always Free** works (x86 shapes run the release binary; Ampere
  ARM compiles from source with Docker), but Oracle reclaims long-idle free
  instances — run a small companion service, or switch the account to
  pay-as-you-go, to keep it.
- **Fly.io / Railway / Hetzner / a VPS** all work the same way: one process,
  one SQLite file, Caddy in front.
- A free name: [DuckDNS](https://www.duckdns.org) — point it at the host's IP,
  pass it to `--domain`, and Caddy gets the certificate.

## What the server can see

By design (ADR-0007/0008): your email, vault and device ids, blob paths and
sizes (`items/<note>/<device>/<n>.enc`), and when devices connect. Not note
contents, titles, keys or the password — sign-in sends a verifier derived from
it, and the stored hash of that verifier cannot be used to sign in. Pairing
codes and device tokens are stored hashed; a leaked database still cannot
decrypt anything.
