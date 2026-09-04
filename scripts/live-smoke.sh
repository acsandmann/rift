#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RIFT_BIN="${RIFT_BIN:-$ROOT/target/debug/rift}"
RIFT_CLI_BIN="${RIFT_CLI_BIN:-$ROOT/target/debug/rift-cli}"
# Use brew fallback if debug not built
if [[ ! -x "$RIFT_CLI_BIN" ]]; then RIFT_CLI_BIN="/opt/homebrew/bin/rift-cli"; RIFT_BIN="/opt/homebrew/bin/rift"; fi
JQ="${JQ:-/opt/homebrew/bin/jq}"

# Allow custom Mach name for ephemeral test instance (avoids clobbering your real rift)
# If RIFT_BS_NAME is set, both rift and rift-cli will use it (see src/sys/mach.rs bs_name())
export RIFT_BS_NAME="${RIFT_BS_NAME:-}"

MODE="${1:-live}" # live | ephemeral

echo "== rift live smoke (rift-cli) =="
echo "ROOT=$ROOT"
echo "RIFT_BIN=$RIFT_BIN"
echo "RIFT_CLI_BIN=$RIFT_CLI_BIN"
echo "RIFT_BS_NAME=${RIFT_BS_NAME:-<default git.acsandmann.rift>}"
echo "MODE=$MODE"
echo ""

need_jq() {
  if ! command -v "$JQ" >/dev/null 2>&1; then echo "jq not found at $JQ — install via brew install jq"; exit 1; fi
}

rift_cli() {
  # Always go through cargo-built binary so protocol matches current build when possible
  if [[ -x "$ROOT/target/debug/rift-cli" ]]; then
    "$ROOT/target/debug/rift-cli" "$@"
  else
    "$RIFT_CLI_BIN" "$@"
  fi
}

check_service() {
  echo "[1/5] Checking Mach service"
  if launchctl print "gui/$(id -u)/git.acsandmann.rift" >/dev/null 2>&1; then
    launchctl print "gui/$(id -u)/git.acsandmann.rift" | grep -E "state|pid|port|active count" | head -n 10
  else
    echo "No LaunchAgent git.acsandmann.rift found — is rift installed via 'rift service install'?"
  fi
  echo ""
  echo "Trying: rift-cli query displays"
  if rift_cli query displays 2>&1 | tee /tmp/rift-live-smoke-displays.json; then
    echo "✓ Mach reachable"
  else
    echo "✗ Mach not reachable — see hint above"
    echo "  If you just built a new rift, run: ./scripts/rift-dev-rebuild.sh  (needs codesign identity 'Rift Dev')"
    echo "  Or run ephemeral mode: RIFT_BS_NAME=git.acsandmann.rift.test ./scripts/live-smoke.sh ephemeral"
    return 1
  fi
  echo ""
}

query_and_assert() {
  local label="$1"; shift
  echo "→ $label: $*"
  local out
  if ! out=$(rift_cli "$@" 2>&1); then
    echo "  ✗ failed: $out"
    return 1
  fi
  echo "$out" | head -n 20
  echo "$out" > "/tmp/rift-live-$(echo "$label" | tr ' ' '-' | tr '/' '-').json"
  echo "  ✓"
  echo ""
}

ephemeral_run() {
  local test_bs="git.acsandmann.rift.test"
  export RIFT_BS_NAME="$test_bs"
  echo "[ephemeral] Starting $RIFT_BIN with RIFT_BS_NAME=$test_bs"
  echo "  Logs: /tmp/rift-ephemeral.out.log /tmp/rift-ephemeral.err.log"
  # Kill any prior ephemeral
  pkill -f "rift.*$test_bs" 2>/dev/null || true
  # Start rift in background, detached from launchd, with no-animate for test
  RUST_LOG=info RIFT_BS_NAME="$test_bs" "$RIFT_BIN" --no-animate > /tmp/rift-ephemeral.out.log 2> /tmp/rift-ephemeral.err.log &
  local pid=$!
  echo "  pid=$pid"
  # Wait for Mach registration
  for i in $(seq 1 10); do
    if RIFT_BS_NAME="$test_bs" "$RIFT_CLI_BIN" query displays >/dev/null 2>&1; then
      echo "  ✓ ephemeral Mach registered after $i tries"
      break
    fi
    sleep 0.5
    if ! kill -0 $pid 2>/dev/null; then echo "  ✗ rift died early — see /tmp/rift-ephemeral.err.log"; cat /tmp/rift-ephemeral.err.log | tail -n 40; return 1; fi
    if [[ $i == 10 ]]; then echo "  ✗ timeout waiting for Mach"; cat /tmp/rift-ephemeral.err.log | tail -n 40; kill $pid 2>/dev/null || true; return 1; fi
  done
  echo ""
  # Run same smoke against ephemeral
  RIFT_BS_NAME="$test_bs" "$RIFT_CLI_BIN" query displays | tee /tmp/rift-ephemeral-displays.json
  RIFT_BS_NAME="$test_bs" "$RIFT_CLI_BIN" query workspaces | tee /tmp/rift-ephemeral-workspaces.json | head -n 40
  RIFT_BS_NAME="$test_bs" "$RIFT_CLI_BIN" query windows | tee /tmp/rift-ephemeral-windows.json | head -n 40
  echo ""
  echo "Ephemeral smoke done — killing $pid"
  kill $pid 2>/dev/null || true
  wait $pid 2>/dev/null || true
  echo "✓ ephemeral done"
}

# ---- main ----
if [[ "$MODE" == "ephemeral" ]]; then
  need_jq
  ephemeral_run
  exit 0
fi

# Live mode
if ! check_service; then
  echo ""
  echo "Live Mach not reachable — falling back to headless harness results:"
  echo "  ./scripts/autonomous-test.sh"
  exit 1
fi

need_jq
echo "[2/5] Displays"
query_and_assert "displays" query displays
echo "[3/5] Workspaces"
query_and_assert "workspaces" query workspaces
echo "[4/5] Windows"
query_and_assert "windows" query windows
echo "[5/5] Live execute smoke (non-destructive)"

# Use execute debug/serialize which are read-only
echo "→ execute debug (layout tree)"
if rift_cli execute debug 2>&1 | head -n 40; then echo "  ✓ debug ok"; else echo "  ✗ debug failed"; fi
echo ""
echo "→ execute serialize (runtime state)"
if rift_cli execute serialize 2>&1 | head -n 20; then echo "  ✓ serialize ok"; else echo "  ✗ serialize failed"; fi
echo ""

# If >1 display, try display move on focused window (dry-run, will move if you have focus)
DISPLAY_COUNT=$(rift_cli query displays 2>&1 | "$JQ" 'length' 2>/dev/null || echo 0)
echo "Displays: $DISPLAY_COUNT"
if [[ "$DISPLAY_COUNT" -gt 1 ]]; then
  echo "→ execute display move-window --direction right (focused window)"
  echo "  (this WILL move your focused window if rift is active — skip with SKIP_DISPLAY_MOVE=1)"
  if [[ "${SKIP_DISPLAY_MOVE:-0}" == "1" ]]; then echo "  skipped"; else
    if rift_cli execute display move-window --direction right 2>&1; then
      echo "  ✓ move ok — verifying via query windows"
      sleep 0.5
      rift_cli query windows | head -n 40
      echo "  → moving back left"
      rift_cli execute display move-window --direction left 2>&1 || true
    else
      echo "  ✗ move failed (maybe no focused window or single space)"
    fi
  fi
else
  echo "Single display — skipping cross-display move smoke (use RIFT_BS_NAME test with 2 virtual displays via drag_harness)"
fi

echo ""
echo "== live smoke done =="
echo "Artifacts: /tmp/rift-live-*.json /tmp/rift-ephemeral*.log"
echo "For full autonomous coverage without needing a second monitor: ./scripts/autonomous-test.sh"
