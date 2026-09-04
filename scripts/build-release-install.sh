#!/usr/bin/env bash
set -euo pipefail

# Build rift in release and install to ~/.local/bin + ~/Applications/Rift.app
# preserving Accessibility (TCC) grant by signing with stable "Rift Dev" identity
# when available. Ad-hoc ("-") changes CDHash every build and forces re-prompt.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$HOME/.local/bin"
APP="$HOME/Applications/Rift.app"
EXECUTABLE="$APP/Contents/MacOS/rift"

# Pick stable identity if present - keeps TCC across rebuilds
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Rift Dev"; then
  IDENTITY="Rift Dev"
else
  IDENTITY="-"
fi

echo "== rift release build + install =="
echo "Root: $ROOT"
echo "Identity: $IDENTITY (Rift Dev keeps TCC, '-' is ad-hoc and will re-prompt)"
echo "Dest: $DEST"
echo "App: $APP"
echo ""

cd "$ROOT"

echo "[1/4] cargo build --release --locked --bins"
cargo build --release --locked --bins
echo ""

SRC_DIR="target/release"
for bin in rift rift-cli; do
  if [[ ! -f "$SRC_DIR/$bin" ]]; then
    echo "ERROR: $SRC_DIR/$bin not found after build" >&2
    exit 1
  fi
done

echo "[2/4] Install to $DEST"
mkdir -p "$DEST"
for bin in rift rift-cli; do
  src="$SRC_DIR/$bin"
  dst="$DEST/$bin"
  echo "  cp $src -> $dst"
  cp -f "$src" "$dst"
  chmod +x "$dst"
  # rift binary must keep identifier git.acsandmann.rift for TCC;
  # rift-cli uses its own identifier (does not need TCC but keep distinct)
  if [[ "$bin" == "rift" ]]; then
    ident="git.acsandmann.rift"
  else
    ident="git.acsandmann.rift-cli"
  fi
  echo "  codesign --force --sign \"$IDENTITY\" --identifier $ident --timestamp=none $dst"
  codesign --force --sign "$IDENTITY" --identifier "$ident" --timestamp=none "$dst"
  codesign --verify --strict --verbose=2 "$dst" 2>&1 | head -n 5
done
echo ""

echo "[3/4] Update $APP"
if [[ -d "$APP" ]]; then
  mkdir -p "$(dirname "$EXECUTABLE")"
  echo "  cp $SRC_DIR/rift -> $EXECUTABLE"
  cp -f "$SRC_DIR/rift" "$EXECUTABLE"
  chmod +x "$EXECUTABLE"
  echo "  codesign --force --sign \"$IDENTITY\" --identifier git.acsandmann.rift --timestamp=none $EXECUTABLE"
  codesign --force --sign "$IDENTITY" --identifier git.acsandmann.rift --timestamp=none "$EXECUTABLE"
  echo "  codesign --force --sign \"$IDENTITY\" --timestamp=none $APP"
  codesign --force --sign "$IDENTITY" --timestamp=none "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | head -n 20
  echo "  Rift.app updated"
else
  echo "  SKIP: $APP not found (expected at ~/Applications/Rift.app)"
  echo "  Only ~/.local/bin updated. To create app bundle, run: scripts/build-local.sh --release"
fi
echo ""

echo "[4/4] Verify"
for f in "$DEST"/rift "$DEST"/rift-cli "$EXECUTABLE"; do [[ -e $f ]] && du -h "$f"; done
echo ""
echo "  codesign rift:"
codesign -dv "$DEST/rift" 2>&1 | grep -E "Identifier|TeamIdentifier|Authority|Signature" | head -n 5
echo "  codesign Rift.app:"
codesign -dv "$APP" 2>&1 | grep -E "Identifier|TeamIdentifier|Authority" | head -n 5
echo ""
echo "  which rift -> $(which rift)"
echo "  rift --help | head:"
"$DEST/rift" --help 2>&1 | head -n 5
echo ""
if [[ "$IDENTITY" == "Rift Dev" ]]; then
  echo "✓ Signed with \"Rift Dev\" - TCC should persist (no re-prompt)."
  echo "  If TCC still prompts, check System Settings > Privacy & Security > Accessibility"
  echo "  and ensure \"Rift\" or \"Rift.app\" is allowed (single entry, not duplicated)."
else
  echo "⚠ Signed ad-hoc (\"-\") - TCC will re-prompt on next launch."
  echo "  To fix: create a self-signed \"Rift Dev\" cert:"
  echo "    Keychain Access > Certificate Assistant > Create Certificate > Name: Rift Dev, Type: Code Signing, Trust: Always Trust"
fi
echo ""
echo "Next: restart service if you updated Rift.app:"
echo "  launchctl bootout gui/\$(id -u) ~/Library/LaunchAgents/git.acsandmann.rift.plist 2>/dev/null || true"
echo "  launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/git.acsandmann.rift.plist"
echo "  launchctl kickstart gui/\$(id -u)/git.acsandmann.rift"
echo "Or just: ./scripts/rift-dev-rebuild.sh (debug) or re-run this script and then kickstart."
