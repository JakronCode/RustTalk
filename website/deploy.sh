#!/usr/bin/env bash
# Uploads the static site to the web server.
# Usage: RUSTTALK_WEB_HOST=root@your-server bash website/deploy.sh
set -euo pipefail

host="${RUSTTALK_WEB_HOST:?set RUSTTALK_WEB_HOST, e.g. root@your-server}"
dest="${RUSTTALK_WEB_ROOT:-/var/www/rusttalk}"
cd "$(dirname "${BASH_SOURCE[0]}")"

files=(index.html roadmap.html impressum.html datenschutz.html legal.css lang.js favicon.svg img)
tar -czf - "${files[@]}" | ssh "$host" "set -e; mkdir -p '$dest'; tar -xzf - -C '$dest' --no-same-owner; chmod -R a+rX '$dest'"
echo "Deployed to $host:$dest"
