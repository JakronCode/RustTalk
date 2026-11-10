#!/usr/bin/env bash
# Lokaler E2E-Smoke-Test auf dem Server selbst:
# startet eine Server-Instanz auf einem freien Port und laesst zwei
# test_client-Instanzen sich gegenseitig Audio schicken.
#
# Aufruf: bash Server/Test_Scripts/e2e_local.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

BIN=target/release/rusttalk-server
TC=target/release/test_client
if [ ! -x "$BIN" ] || [ ! -x "$TC" ]; then
    echo "Release-Build fehlt -> cargo build --release -p rusttalk-server"
    cargo build --release -p rusttalk-server
fi

PORT=17331
CFG=$(mktemp /tmp/rusttalk-e2e-XXXX.toml)
cat > "$CFG" <<EOF
bind_address = "127.0.0.1:$PORT"
server_name = "E2E-Smoke"
EOF

echo "=== Starte Server auf 127.0.0.1:$PORT ==="
RUST_LOG=warn "$BIN" "$CFG" &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; rm -f "$CFG"' EXIT
sleep 0.7

echo "=== Zwei test_clients schicken sich 3 s Audio ==="
"$TC" --addr "127.0.0.1:$PORT" --name E2E-A --audio-secs 3 --duration-secs 6 > /tmp/e2e-a.log 2>&1 &
PID_A=$!
sleep 0.4
"$TC" --addr "127.0.0.1:$PORT" --name E2E-B --audio-secs 3 --duration-secs 6 > /tmp/e2e-b.log 2>&1 &
PID_B=$!
wait $PID_A
wait $PID_B

echo "--- Client A ---"; cat /tmp/e2e-a.log
echo "--- Client B ---"; cat /tmp/e2e-b.log

RX_A=$(grep -oP 'AUDIO_RX=\K\d+' /tmp/e2e-a.log || echo 0)
RX_B=$(grep -oP 'AUDIO_RX=\K\d+' /tmp/e2e-b.log || echo 0)

if [ "$RX_A" -ge 10 ] && [ "$RX_B" -ge 10 ]; then
    echo "E2E OK: A hoerte $RX_A Pakete von B, B hoerte $RX_B Pakete von A"
else
    echo "E2E FEHLGESCHLAGEN (RX_A=$RX_A, RX_B=$RX_B)" >&2
    exit 1
fi
