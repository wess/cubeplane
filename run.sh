#!/usr/bin/env bash
# Launch cubeplane: the Java engine, the Atlas admin panel, and (if Java 21 is
# available) the Bedrock bridge so phones/consoles can join too.
#
#   ./run.sh             release build, everything, Ctrl-C stops it all
#   ./run.sh --debug     faster debug build of the engine
#   ./run.sh --no-bridge skip the Bedrock bridge (Java clients only)
#
# Ports: engine 25565 (Java TCP), panel 3000 (web), control API 8080,
#        bridge 19132 (Bedrock UDP), bridge 25568 (Java passthrough TCP).

set -euo pipefail
cd "$(dirname "$0")"

profile="--release"
bridge=1
for arg in "$@"; do
  case "$arg" in
    --debug) profile="" ;;
    --no-bridge) bridge=0 ;;
  esac
done

VIAPROXY_URL="https://github.com/ViaVersion/ViaProxy/releases/download/v3.4.11/ViaProxy-3.4.11.jar"
GEYSER_URL="https://download.geysermc.org/v2/projects/geyser/versions/latest/builds/latest/downloads/viaproxy"

engine_pid="" ; panel_pid="" ; bridge_pid=""
cleanup() {
  [ -n "$bridge_pid" ] && kill "$bridge_pid" 2>/dev/null || true
  [ -n "$panel_pid" ]  && kill "$panel_pid"  2>/dev/null || true
  [ -n "$engine_pid" ] && kill "$engine_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Locate a Java 21+ runtime for the bridge without disturbing the default java.
find_java21() {
  for c in /opt/homebrew/opt/openjdk@21/bin/java /usr/local/opt/openjdk@21/bin/java; do
    [ -x "$c" ] && { echo "$c"; return; }
  done
  if [ -x /usr/libexec/java_home ]; then
    h=$(/usr/libexec/java_home -v 21 2>/dev/null || true)
    [ -n "$h" ] && [ -x "$h/bin/java" ] && { echo "$h/bin/java"; return; }
  fi
  command -v java >/dev/null 2>&1 || return
  java -version 2>&1 | grep -qE 'version "(2[1-9]|[3-9][0-9])' && command -v java
}

# --- engine ---------------------------------------------------------------
echo "cubeplane → building & starting engine…"
cargo run $profile &
engine_pid=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null http://127.0.0.1:8080/api/status && break
  sleep 1
done

# --- admin panel ----------------------------------------------------------
if [ ! -d admin/node_modules ]; then
  echo "cubeplane → installing admin panel deps…"
  (cd admin && bun install)
fi
echo "cubeplane → starting admin panel…"
(cd admin && bun run start) &
panel_pid=$!

# --- Bedrock bridge (optional) -------------------------------------------
if [ "$bridge" = 1 ]; then
  java21=$(find_java21 || true)
  if [ -z "$java21" ]; then
    echo "cubeplane → Bedrock bridge SKIPPED: Java 21 not found."
    echo "            Install it with:  brew install openjdk@21"
    echo "            (Java Edition clients still work on port 25565.)"
  else
    mkdir -p bridge/plugins
    [ -f bridge/viaproxy.jar ]      || { echo "cubeplane → downloading ViaProxy…"; curl -sL -o bridge/viaproxy.jar "$VIAPROXY_URL"; }
    [ -f bridge/plugins/geyser.jar ] || { echo "cubeplane → downloading Geyser…";   curl -sL -A cubeplane -o bridge/plugins/geyser.jar "$GEYSER_URL"; }
    echo "cubeplane → starting Bedrock bridge (Geyser via ViaProxy)…"
    ( cd bridge && exec "$java21" -jar viaproxy.jar cli \
        --target-address 127.0.0.1:25565 \
        --target-version "1.20-1.20.1" \
        --bind-address 0.0.0.0:25568 ) &
    bridge_pid=$!
  fi
fi

echo
echo "  engine:  Minecraft Java    →  this machine, port 25565"
echo "  panel:   web dashboard     →  http://localhost:3000"
[ -n "$bridge_pid" ] && echo "  bridge:  Bedrock (phones)  →  this machine's LAN IP, port 19132 (UDP)"
echo "  (Ctrl-C stops everything)"
echo

wait
