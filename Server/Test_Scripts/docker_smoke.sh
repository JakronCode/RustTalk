#!/usr/bin/env bash
# Smoke test of the server image with the hardening from the compose example:
# fresh named volume, read-only root, no capabilities, non-root, mounted admin secret.
# Usage: bash Server/Test_Scripts/docker_smoke.sh [image]   (default rusttalk-server:dev)
set -euo pipefail

IMAGE="${1:-rusttalk-server:dev}"
NAME="rusttalk-smoke-$$"
VOLUME="$NAME-data"
TMP="$(mktemp -d)"
SECRET="smoke-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
checks=0

cleanup() {
    docker rm -f "$NAME" "$NAME-bad" "$NAME-ls" >/dev/null 2>&1 || true
    docker volume rm -f "$VOLUME" >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT

ok() { checks=$((checks + 1)); echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; exit 1; }

HARDENING=(--read-only --cap-drop ALL --security-opt no-new-privileges --tmpfs /tmp -v "$VOLUME:/data")
SECRET_MOUNT="/run/secrets/rusttalk_admin_password"

# Files in the volume as "mode uid name" (tar listing, works without a shell in the image).
volume_ls() {
    docker rm -f "$NAME-ls" >/dev/null 2>&1 || true
    docker create --name "$NAME-ls" -v "$VOLUME:/data" "$IMAGE" >/dev/null
    docker cp "$NAME-ls:/data" - | tar --numeric-owner -tvf - | awk '{split($2, o, "/"); print $1, o[1], $NF}'
    docker rm -f "$NAME-ls" >/dev/null
}

wait_healthy() {
    for _ in $(seq 60); do
        case "$(docker inspect -f '{{.State.Health.Status}}' "$NAME")" in
            healthy) return 0 ;;
            unhealthy) break ;;
        esac
        sleep 1
    done
    docker logs "$NAME" >&2 || true
    fail "container did not become healthy"
}

fingerprint() { docker logs "$NAME" 2>&1 | grep -o 'TLS fingerprint: [0-9A-Fa-f:]*' | tail -1; }

docker volume create "$VOLUME" >/dev/null

# --- Fail closed: no secret, then a malformed one. Nothing may be created. ---
if out="$(docker run --rm --name "$NAME-bad" "${HARDENING[@]}" "$IMAGE" 2>&1)"; then
    fail "started without an admin secret"
fi
grep -q "Cannot read secret file $SECRET_MOUNT" <<<"$out" || fail "unexpected error: $out"
ok "refuses to start without an admin secret"

printf 'LEAKME-short\n' > "$TMP/short"
chmod 644 "$TMP/short"
if out="$(docker run --rm --name "$NAME-bad" "${HARDENING[@]}" -v "$TMP/short:$SECRET_MOUNT:ro" "$IMAGE" 2>&1)"; then
    fail "started with a too short admin secret"
fi
grep -q "shorter than" <<<"$out" || fail "unexpected error: $out"
grep -q "LEAKME" <<<"$out" && fail "secret content in the output"
ok "refuses a too short secret without printing it"
[ -z "$(volume_ls | grep -v ' data/$')" ] || fail "volume not empty after refused starts: $(volume_ls)"
ok "refused starts leave the volume empty"

# --- First start on the fresh volume ---
# The container user (65532) must be able to read the bind-mounted secret; test-only secret.
printf '%s\n' "$SECRET" > "$TMP/admin_password"
chmod 644 "$TMP/admin_password"
docker run -d --name "$NAME" "${HARDENING[@]}" \
    -v "$TMP/admin_password:$SECRET_MOUNT:ro" \
    -p 127.0.0.1::7331/tcp -p 127.0.0.1::7331/udp "$IMAGE" >/dev/null
wait_healthy
ok "healthy on a fresh named volume"

[ "$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}} {{.HostConfig.CapDrop}} {{.HostConfig.SecurityOpt}}' "$NAME")" \
    = "true [ALL] [no-new-privileges]" ] || fail "hardening flags not applied"
uids="$(docker top "$NAME" -eo pid,uid | awk 'NR > 1 {print $2}' | sort -u)"
[ "$uids" = "65532" ] || fail "processes run as uid '$uids', expected 65532"
ok "runs as uid 65532 with read-only root and no capabilities"

listing="$(volume_ls)"
for want in "drwx------ 65532 data/" "-rw------- 65532 data/server.toml" "-rw------- 65532 data/tls-key.der" "-rw------- 65532 data/tls-cert.der"; do
    grep -qxF -- "$want" <<<"$listing" || fail "missing '$want' in volume: $listing"
done
grep -q " data/rusttalk.db$" <<<"$listing" || fail "no database in volume: $listing"
ok "volume holds config, database and TLS key/cert (0600, owner 65532)"

port="$(docker port "$NAME" 7331/tcp | head -1 | sed 's/.*://')"
(exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null || fail "published TCP port $port not reachable"
ok "TCP 7331 reachable through the published port"

docker exec "$NAME" /usr/local/bin/rusttalk-server --healthcheck /data/server.toml || fail "healthcheck exit code"
ok "healthcheck subcommand reports healthy"

if out="$(docker exec "$NAME" /usr/local/bin/rusttalk-server update 2>&1)"; then
    fail "update install was not refused"
fi
grep -q "pulling the new image" <<<"$out" || fail "unexpected update output: $out"
ok "in-container update install is refused"

docker logs "$NAME" 2>&1 | grep -qF "$SECRET" && fail "admin secret in the logs"
ok "admin secret not in the logs"

fp1="$(fingerprint)"
[ -n "$fp1" ] || fail "no TLS fingerprint in the logs"
config_sum="$(docker cp "$NAME:/data/server.toml" - | sha256sum)"

# --- docker stop: SIGTERM handled, clean exit well before the 10 s kill ---
start=$(date +%s)
docker stop "$NAME" >/dev/null
elapsed=$(( $(date +%s) - start ))
[ "$(docker inspect -f '{{.State.ExitCode}}' "$NAME")" = "0" ] || fail "exit code $(docker inspect -f '{{.State.ExitCode}}' "$NAME") after docker stop"
[ "$elapsed" -lt 8 ] || fail "docker stop took ${elapsed}s (SIGTERM ignored?)"
docker logs "$NAME" 2>&1 | grep -q "Shutdown complete" || fail "no clean shutdown in the logs"
volume_ls | grep -q "rusttalk.db-wal" && fail "WAL left behind: database not closed cleanly"
ok "docker stop: SIGTERM, database closed, exit code 0 in ${elapsed}s"

# --- Restart on the same volume: state kept, nothing regenerated ---
docker start "$NAME" >/dev/null
wait_healthy
[ "$(fingerprint)" = "$fp1" ] || fail "TLS fingerprint changed: $fp1 -> $(fingerprint)"
[ "$(docker cp "$NAME:/data/server.toml" - | sha256sum)" = "$config_sum" ] || fail "server.toml changed on restart"
[ "$(docker logs "$NAME" 2>&1 | grep -c 'Created default config')" = "1" ] || fail "config created twice"
[ "$(docker logs "$NAME" 2>&1 | grep -c 'Created new TLS certificate')" = "1" ] || fail "certificate created twice"
ok "restart keeps config, data and TLS fingerprint"

echo ""
echo "DOCKER SMOKE TEST OK ($checks checks, image $IMAGE)"
