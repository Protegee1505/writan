# Self-hosting the Writan sync server

`writan-server` is the optional third sync target (after a shared folder and an
S3-compatible bucket). It is a *dumb* ciphertext store: it keeps the encrypted
blobs of your vaults, hands devices their tokens and streams change events. It
never sees a note, a title, a vault key or a password you type into Writan —
the vault key stays on your devices, and notes are end-to-end encrypted before
they leave them. The decisions behind it are ADR-0007
(`docs/adr/0007-noteserver.md`).

You do not need a server: folder and S3 sync work without one. Pick one when you
want your own always-on target, for example so phones can sync without a cloud
bucket.

## Quick start

```sh
git clone https://github.com/Protegee1505/writan
cd writan
docker compose up -d --build
curl http://127.0.0.1:8787/health
# {"status":"ok","version":"0.1.0","schema":1}
```

Or with plain Docker:

```sh
docker build -t writan-server .
docker run -d --name writan-server --restart unless-stopped \
  -p 127.0.0.1:8787:8787 -v writan-data:/data writan-server
```

A tagged release also publishes an amd64 image:

```sh
docker run -d --name writan-server --restart unless-stopped \
  -p 127.0.0.1:8787:8787 -v writan-data:/data \
  ghcr.io/protegee1505/writan-server:latest
```

On an ARM host (for example Oracle's free Ampere VMs) build it from source with
`docker compose up -d --build`; the published image is amd64 only.

## Connecting the app

In Writan: **Settings → Sync**, choose **Writan server**, enter the address
(`https://notes.example.com`), your email and an account password, and leave
**Create the account** on the first time. The account password protects
sign-in; the *sync password* below it protects your vault key and should be
different. A second device either signs in with the same account password, or
uses **Pair a device** on the first device and **Have a pairing code?** on the
new one — a one-time code (10 minutes) that grants blob access only, never the
vault key.

## Configuration

Everything is environment variables; there is no config file.

| Variable | Default | Meaning |
| --- | --- | --- |
| `WRITAN_PORT` | `8787` | Listen port (all interfaces). |
| `WRITAN_DATA_DIR` | `writan-data` | Where `writan-server.sqlite` lives. |
| `WRITAN_REGISTRATION` | `open` | `closed` refuses new accounts. |
| `WRITAN_MAX_BLOB_KIB` | `4096` | Largest single blob; bigger uploads get 413. |
| `WRITAN_VAULT_QUOTA_MIB` | `512` | Per-vault quota; `0` = unlimited; over quota gets 507. |
| `WRITAN_PAIRING_TTL_SECS` | `600` | Lifetime of a pairing code. |

## TLS

The server speaks plain HTTP and expects a reverse proxy to terminate TLS.
With [Caddy](https://caddyserver.com) on the same host, the whole config is:

```
notes.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Caddy gets a certificate automatically. Do not expose the port directly to the
internet — without TLS, tokens and encrypted blobs travel in the clear.

## Accounts and admin

- Create the first account from the app (leave **Create the account** on).
  Afterwards `WRITAN_REGISTRATION=closed` keeps anyone else out.
- **Forgot the account password** (there is no email; reset from the host):

  ```sh
  docker exec -it writan-server writan-server reset-password you@example.com
  # non-interactively:
  docker exec -e WRITAN_NEW_PASSWORD='new account password' writan-server \
    writan-server reset-password you@example.com
  ```

  This revokes every device token of that account; devices sign in again.
- Five failed sign-ins lock an account for 60 s, doubling up to 15 minutes.

## Backups and moving hosts

The whole server state is one SQLite file:

```sh
docker stop writan-server
docker run --rm -v writan-data:/data -v "$PWD":/backup debian:bookworm-slim \
  cp /data/writan-server.sqlite /backup/writan-server.sqlite
docker start writan-server
```

Restore by putting the file back into the volume (fresh host, same
`docker-compose.yml`) and starting the server. Devices keep working; nothing on
the clients changes.

## Upgrades

`docker compose build --pull && docker compose up -d`, or pull a newer tag. The
store carries its schema version; the server refuses to start when it does not
match the binary (there is no migration path yet), so keep the matching image
and take a backup before switching versions.

## Limits and housekeeping

- Blobs are immutable and are never deleted (no garbage collection in v1): a
  deleted note is a small tombstone, so vaults grow slowly and the quota is the
  guard rail. Raise `WRITAN_VAULT_QUOTA_MIB` if a big vault hits it.
- Everything is scoped to one account: one vault per account, and a vault id
  belonging to another account answers 404.
- `GET /health` answers `{"status":"ok","version":…,"schema":…}`; logs go to the
  container's stdout.

## Small hosts and free tiers

The server idles at a few tens of megabytes and needs no cache or queue, so a
1 vCPU / 1 GB VM is plenty.

- **Oracle Cloud Always Free** works (Ampere ARM or the x86 shapes) but Oracle
  reclaims long-idle free instances — run a small companion service, or switch
  the account to pay-as-you-go, to keep it.
- **Fly.io / Railway / Hetzner / a VPS** all work the same way: one container,
  one volume, Caddy in front.
- **ARM vs amd64**: build from source on ARM (`docker compose up -d --build`).

## What the server can see

By design (ADR-0007): your email, vault and device ids, blob paths and sizes
(`items/<note>/<device>/<n>.enc`), and when devices connect. Not note contents,
titles, or keys. Pairing codes and device tokens are stored hashed; a leaked
database still cannot decrypt anything.
