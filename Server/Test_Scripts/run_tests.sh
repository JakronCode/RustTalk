#!/usr/bin/env bash
# Alle Server-Tests auf dem Linux-Server ausführen.
# Aufruf: bash Server/Test_Scripts/run_tests.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

echo "=== Unit-Tests (shared + server) ==="
cargo test -p rusttalk-shared -p rusttalk-server --lib

echo ""
echo "=== Integrationstests (echter Server-Prozess + synthetische Clients) ==="
cargo test -p rusttalk-server --test integration

echo ""
echo "ALLE TESTS OK"
