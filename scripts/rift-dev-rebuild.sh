#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$HOME/Applications/Rift.app"
EXECUTABLE="$APP/Contents/MacOS/rift"
# Use stable "Rift Dev" cert if available – keeps Accessibility (TCC) grant
# across rebuilds. Adhoc ("-") changes CDHash every build, forcing a re-prompt.
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Rift Dev"; then
  IDENTITY="Rift Dev"
else
  IDENTITY="-"
fi
SERVICE="gui/$(id -u)/git.acsandmann.rift"
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/git.acsandmann.rift.plist"

cd "$ROOT"
cargo build --locked --bins
mkdir -p "$(dirname "$EXECUTABLE")"
cp target/debug/rift "$EXECUTABLE"
codesign --force --sign "$IDENTITY" --identifier git.acsandmann.rift --timestamp=none "$EXECUTABLE"
codesign --force --sign "$IDENTITY" --timestamp=none "$APP"
# Also keep ~/.local/bin in sync and signed with same identity (for `which rift`)
if [[ -d "$HOME/.local/bin" ]]; then
  cp target/debug/rift "$HOME/.local/bin/rift" 2>/dev/null || true
  cp target/debug/rift-cli "$HOME/.local/bin/rift-cli" 2>/dev/null || true
  codesign --force --sign "$IDENTITY" --identifier git.acsandmann.rift --timestamp=none "$HOME/.local/bin/rift" 2>/dev/null || true
  codesign --force --sign "$IDENTITY" --identifier git.acsandmann.rift --timestamp=none "$HOME/.local/bin/rift-cli" 2>/dev/null || true
fi
codesign --verify --deep --strict --verbose=2 "$APP"
launchctl bootout "$DOMAIN" "$PLIST" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart "$SERVICE"

echo "Rebuilt, signed, and restarted $APP"
