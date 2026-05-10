#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Codex Sentinel"
APP_DIR="$ROOT/dist/${APP_NAME}.app"
SIGNING_IDENTITY="${CODEX_SENTINEL_SIGNING_IDENTITY:--}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
ARCH="$(uname -m | sed 's/^arm64$/aarch64/;s/^x86_64$/x64/')"

cd "$ROOT"

if [ ! -d "$ROOT/node_modules" ]; then
  npm install
fi

npm run tauri -- build --bundles app

rm -rf "$ROOT/dist"
mkdir -p "$ROOT/dist"

BUNDLE_APP="$ROOT/target/release/bundle/macos/${APP_NAME}.app"
if [ ! -d "$BUNDLE_APP" ]; then
  echo "Tauri did not produce $BUNDLE_APP" >&2
  exit 1
fi

cp -R "$BUNDLE_APP" "$APP_DIR"
codesign --force --deep --sign "$SIGNING_IDENTITY" --identifier local.codex-sentinel "$APP_DIR" >/dev/null
hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$APP_DIR" \
  -ov \
  -format UDZO \
  "$ROOT/dist/${APP_NAME}_${VERSION}_${ARCH}.dmg" >/dev/null

echo "$APP_DIR"
