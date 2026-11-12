#!/usr/bin/env bash
# All server tests: shared library + server binaries (unit) and the integration
# tests (real server process + synthetic clients). Serial, like CI.
# Usage: bash Server/Test_Scripts/run_tests.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

cargo test --locked -p rusttalk-shared -p rusttalk-server -- --test-threads=1

echo ""
echo "ALL SERVER TESTS OK"
