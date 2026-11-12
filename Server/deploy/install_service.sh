#!/usr/bin/env bash
# Installiert den RustTalk-Server als systemd-Service auf dem Debian-Ziel.
# Voraussetzung: Release-Build existiert (cargo build --release -p rusttalk-server).
# Aufruf als root auf dem Server:  bash Server/deploy/install_service.sh
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALL_DIR=/opt/rusttalk/server

# Unprivilegierter Service-User
id -u rusttalk >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin rusttalk

install -d -m 755 "$INSTALL_DIR/bin"
install -m 755 "$SRC_DIR/target/release/rusttalk-server" "$INSTALL_DIR/bin/"
install -m 755 "$SRC_DIR/target/release/test_client" "$INSTALL_DIR/bin/"
[ -f "$INSTALL_DIR/server.toml" ] || install -m 644 "$SRC_DIR/Server/server.toml" "$INSTALL_DIR/server.toml"
install -m 644 "$SRC_DIR/Server/deploy/rusttalk-server.service" /etc/systemd/system/

systemctl daemon-reload
systemctl enable --now rusttalk-server
systemctl --no-pager --full status rusttalk-server | head -12 || true

echo "Done. Server listens on 0.0.0.0:7331 (TCP+UDP)."
