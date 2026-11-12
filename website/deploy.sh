#!/usr/bin/env bash
# Uploads the static site to the web server.
# Usage: RUSTTALK_WEB_HOST=user@your-server [RUSTTALK_WEB_PORT=22] bash website/deploy.sh
# The SSH user needs write access to the web root (RUSTTALK_WEB_ROOT, default /var/www/rusttalk).
set -euo pipefail

host="${RUSTTALK_WEB_HOST:?set RUSTTALK_WEB_HOST, e.g. user@your-server}"
port="${RUSTTALK_WEB_PORT:-22}"
dest="${RUSTTALK_WEB_ROOT:-/var/www/rusttalk}"
cd "$(dirname "${BASH_SOURCE[0]}")"

files=(index.html roadmap.html impressum.html datenschutz.html legal.css lang.js favicon.svg img)
tar -czf - "${files[@]}" | ssh -p "$port" "$host" "set -e; mkdir -p '$dest'; tar -xzf - -C '$dest' --no-same-owner; chmod -R a+rX '$dest'"
echo "Deployed to $host:$dest"
