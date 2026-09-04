#!/usr/bin/env bash
set -euo pipefail

# Build rift from this repo and install to ~/.local/bin, codesigned.
# Unlinks brew shims via PATH precedence (~/.local/bin is before /opt/homebrew/bin).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$HOME/.local/bin"
IDENTITY="-"
BINS=(rift rift-cli)

cd "$ROOT"
if [[ "${1:-}" == "--release" ]]; then
  echo "[build-local] cargo build --release --locked --bins"
  cargo build --release --locked --bins
  SRC_DIR="target/release"
else
  echo "[build-local] cargo build --locked --bins (debug, use --release for release)"
  cargo build --locked --bins
  SRC_DIR="target/debug"
fi

mkdir -p "$DEST"
for bin in "${BINS[@]}"; do
  src="$SRC_DIR/$bin"
  dst="$DEST/$bin"
  echo "[build-local] cp $src -> $dst"
  cp -f "$src" "$dst"
  chmod +x "$dst"
  echo "[build-local] codesign --force --sign $IDENTITY $dst"
  codesign --force --sign "$IDENTITY" --identifier "git.acsandmann.$bin" --timestamp=none "$dst"
  codesign --verify --strict --verbose=2 "$dst"
done

echo "[build-local] also updating Rift.app"
APP="$HOME/Applications/Rift.app"
if [[ -d "$APP" ]]; then
  cp -f "$DEST/rift" "$APP/Contents/MacOS/rift"
  codesign --force --sign - --identifier git.acsandmann.rift --timestamp=none "$APP/Contents/MacOS/rift"
  codesign --force --sign - --timestamp=none "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
  echo "[build-local] Rift.app updated and signed"
fi

echo "[build-local] verify"
ls -lh "$DEST"/rift "$DEST"/rift-cli
codesign -dv "$DEST/rift" 2>&1 | head -n 5
codesign -dv "$DEST/rift-cli" 2>&1 | head -n 5
echo "[build-local] which rift -> $(which rift)"
echo "[build-local] rift --help | head"
"$DEST/rift" --help | head -n 5
echo "[build-local] done. ~/.local/bin now shadows /opt/homebrew/bin (PATH order)."
echo "          brew rift remains installed but unlinked via PATH precedence."
echo "          To fully unlink when sandbox allows: brew unlink rift"
