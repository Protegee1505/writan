#!/usr/bin/env bash
# Writan sync server installer — one command on a fresh VPS (ADR-0008 §9).
#
#   curl -fsSL https://github.com/Protegee1505/writan/releases/latest/download/writan-server-install.sh \
#     | sudo bash -s -- --domain notes.example.com
#
# Downloads the pinned writan-server release asset, checks it against the
# release's SHA256SUMS.txt, installs /usr/local/bin/writan-server, creates the
# `writan` system user with /var/lib/writan-server, writes
# /etc/writan-server.env (mode 600) and a systemd unit, enables and starts the
# service, and prints the address plus /health. Re-running upgrades the binary
# in place — the data directory is never touched.
#
# Flags:
#   --domain HOST              serve HTTPS for HOST through Caddy; needs caddy
#                              installed (https://caddyserver.com/docs/install).
#                              The server then listens on 127.0.0.1 only.
#   --local                    plain HTTP on every interface (no Caddy)
#   --registration open|closed whether new accounts may be created (open)
#   --port N                   listen port (8787)
#   --version vX.Y.Z           release to install (the default is the release
#                              this script was published with)
set -euo pipefail

REPO="Protegee1505/writan"
VERSION="${WRITAN_VERSION:-v0.2.0}"
DOMAIN=""
LOCAL=0
REGISTRATION="open"
PORT="8787"

usage() {
  cat <<'TXT'
Usage: writan-server-install.sh [--domain HOST] [--local] [--registration open|closed]
                                [--port N] [--version vX.Y.Z]

  --domain HOST              serve HTTPS for HOST through Caddy; needs caddy
                             installed (https://caddyserver.com/docs/install);
                             the server then listens on 127.0.0.1 only
  --local                    plain HTTP on every interface (no Caddy, default)
  --registration open|closed whether new accounts may be created (open)
  --port N                   listen port (8787)
  --version vX.Y.Z           release to install
TXT
}

while [ $# -gt 0 ]; do
  case "$1" in
    --domain) DOMAIN="${2:-}"; shift 2 ;;
    --local) LOCAL=1; shift ;;
    --registration) REGISTRATION="${2:-}"; shift 2 ;;
    --port) PORT="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown flag: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ "$(id -u)" -eq 0 ] || { echo "run me as root: sudo $0 $*" >&2; exit 1; }
[ "$(uname -m)" = "x86_64" ] || { echo "only x86_64 has release builds so far" >&2; exit 1; }
case "$REGISTRATION" in open|closed) ;; *) echo "--registration is open or closed" >&2; exit 2 ;; esac
[ -z "$DOMAIN" ] || [ "$LOCAL" -eq 0 ] || { echo "--domain and --local contradict each other" >&2; exit 2; }
command -v curl >/dev/null || { echo "curl is needed" >&2; exit 1; }
command -v systemctl >/dev/null || { echo "systemd is needed" >&2; exit 1; }

BASE="https://github.com/$REPO/releases/download/$VERSION"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "== download $VERSION"
curl -fsSL -o "$WORK/writan-server" "$BASE/writan-server"
curl -fsSL -o "$WORK/SHA256SUMS.txt" "$BASE/SHA256SUMS.txt"

echo "== verify"
WANT="$(awk '$2 ~ /(^|\/)writan-server$/ { print $1 }' "$WORK/SHA256SUMS.txt")"
[ -n "$WANT" ] || { echo "SHA256SUMS.txt has no writan-server line" >&2; exit 1; }
GOT="$(sha256sum "$WORK/writan-server" | awk '{ print $1 }')"
[ "$WANT" = "$GOT" ] || { echo "checksum mismatch: want $WANT, got $GOT" >&2; exit 1; }

echo "== install /usr/local/bin/writan-server"
install -m 755 "$WORK/writan-server" /usr/local/bin/writan-server

echo "== user and data directory"
id -u writan >/dev/null 2>&1 || useradd --system --home /var/lib/writan-server \
  --shell /usr/sbin/nologin writan
install -d -o writan -g writan -m 700 /var/lib/writan-server

echo "== /etc/writan-server.env"
{
  echo "# Written by writan-server-install.sh; edit and restart the service."
  echo "WRITAN_DATA_DIR=/var/lib/writan-server"
  echo "WRITAN_PORT=$PORT"
  echo "WRITAN_REGISTRATION=$REGISTRATION"
  if [ -n "$DOMAIN" ]; then
    echo "# Caddy terminates TLS and proxies from localhost."
    echo "WRITAN_BIND=127.0.0.1"
  else
    echo "# Plain HTTP on every interface (--local or the default)."
    echo "WRITAN_BIND=0.0.0.0"
  fi
} > /etc/writan-server.env
chmod 600 /etc/writan-server.env

echo "== systemd unit"
cat > /etc/systemd/system/writan-server.service <<'UNIT'
[Unit]
Description=Writan sync server
Documentation=https://github.com/Protegee1505/writan/blob/main/docs/SERVER.md
After=network-online.target
Wants=network-online.target

[Service]
User=writan
Group=writan
EnvironmentFile=/etc/writan-server.env
ExecStart=/usr/local/bin/writan-server serve
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=/var/lib/writan-server

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload

if [ -n "$DOMAIN" ]; then
  echo "== Caddy site for $DOMAIN"
  if ! command -v caddy >/dev/null; then
    echo "caddy is not installed yet; install it, then run this script again:" >&2
    echo "  https://caddyserver.com/docs/install" >&2
    exit 1
  fi
  CADDYFILE=/etc/caddy/Caddyfile
  [ -f "$CADDYFILE" ] || : > "$CADDYFILE"
  BEGIN="# writan-server: begin"
  END="# writan-server: end"
  TMP="$(mktemp)"
  awk -v begin="$BEGIN" -v end="$END" '
    $0 == begin { skip = 1 }
    skip && $0 == end { skip = 0; next }
    !skip { print }
  ' "$CADDYFILE" > "$TMP"
  {
    echo "$BEGIN"
    echo "$DOMAIN {"
    echo "	reverse_proxy 127.0.0.1:$PORT"
    echo "}"
    echo "$END"
  } >> "$TMP"
  mv "$TMP" "$CADDYFILE"
  systemctl enable caddy >/dev/null 2>&1 || true
  systemctl restart caddy
fi

echo "== start the service"
systemctl enable writan-server >/dev/null
systemctl restart writan-server
sleep 1
systemctl is-active --quiet writan-server || {
  echo "the service did not stay up; journalctl -u writan-server" >&2
  exit 1
}

if [ -n "$DOMAIN" ]; then
  ADDRESS="https://$DOMAIN"
else
  HOST_IP="$(curl -fsS -m 5 https://api.ipify.org 2>/dev/null || echo '<this host>')"
  ADDRESS="http://$HOST_IP:$PORT"
fi
echo "== health"
curl -fsS -m 10 "${ADDRESS}/health" || echo "(health not reachable yet — first HTTPS certificate can take a minute)"

cat <<EOF

writan-server $VERSION is running.

In the app: Settings → Sync → Writan server
  address:  $ADDRESS
  email:    you@example.com
  password: the one password (at least 8 characters); the server only ever
            sees a verifier derived from it

Health: $ADDRESS/health
Data:   /var/lib/writan-server/writan-server.sqlite (back up this file)
Logs:   journalctl -u writan-server -f
Reset a password or migrate a pre-v0.2 account:
  WRITAN_DATA_DIR=/var/lib/writan-server writan-server reset-password EMAIL
EOF
