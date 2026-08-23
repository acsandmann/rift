#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo "== rift autonomous test harness =="
echo "Root: $ROOT"
echo ""

# 1. Format check (if nightly available)
if rustup toolchain list 2>/dev/null | grep -q nightly; then
  echo "[1/4] cargo +nightly fmt --all --check --verbose"
  cargo +nightly fmt --all --check --verbose
else
  echo "[1/4] SKIP fmt (nightly not installed) — run: rustup toolchain install nightly"
fi
echo ""

# 2. Compile check
echo "[2/4] cargo check --locked"
cargo check --locked
echo ""

# 3. Reactor + layout unit tests (covers drag, space, display)
echo "[3/4] cargo test -- reactor + layout_engine + drag_harness"
cargo test -- --nocapture 2>&1 | tee /tmp/rift-autonomous-test.log
echo ""

# 4. Summary
echo "[4/4] Summary"
if grep -q "FAILED" /tmp/rift-autonomous-test.log; then
  echo "FAILED — see /tmp/rift-autonomous-test.log"
  grep -E "FAILED|failures:" /tmp/rift-autonomous-test.log | head -n 20
  exit 1
else
  PASS=$(grep -c "ok$" /tmp/rift-autonomous-test.log || true)
  echo "PASS — $PASS tests passed — log: /tmp/rift-autonomous-test.log"
fi

# Optional: verbose drag harness only
echo ""
echo "Drag harness (focused): cargo test -- drag_harness -- --nocapture"
cargo test drag_harness -- --nocapture 2>&1 | tail -n 40 || true
