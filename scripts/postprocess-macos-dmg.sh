#!/bin/bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <dmg-directory> <app-name>" >&2
  exit 1
fi

DMG_DIR="$1"
APP_NAME="$2"
SETTINGS_SHORTCUT_NAME="Open_Privacy_Security_Settings.inetloc"

if [[ ! -d "${DMG_DIR}" ]]; then
  echo "DMG directory not found: ${DMG_DIR}" >&2
  exit 1
fi

DMG_PATH="$(find "${DMG_DIR}" -maxdepth 1 -type f -name "${APP_NAME}_*.dmg" | head -n 1)"
if [[ -z "${DMG_PATH}" ]]; then
  echo "No DMG found for ${APP_NAME} in ${DMG_DIR}" >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
RW_DMG="${TMP_DIR}/${APP_NAME}-rw.dmg"
MOUNT_DIR="${TMP_DIR}/mount"
FINAL_DMG="${TMP_DIR}/$(basename "${DMG_PATH}")"

cleanup() {
  if mount | grep -q "on ${MOUNT_DIR} "; then
    hdiutil detach "${MOUNT_DIR}" -quiet || true
  fi
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

mkdir -p "${MOUNT_DIR}"

hdiutil convert "${DMG_PATH}" -format UDRW -o "${RW_DMG}" -quiet
hdiutil attach "${RW_DMG}" -mountpoint "${MOUNT_DIR}" -nobrowse -quiet

cat > "${MOUNT_DIR}/${SETTINGS_SHORTCUT_NAME}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>URL</key>
  <string>x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension</string>
</dict>
</plist>
EOF

cat > "${MOUNT_DIR}/README-If-macOS-blocks-launch.txt" <<EOF
If macOS blocks ${APP_NAME}, do this:
1. Drag ${APP_NAME}.app into /Applications first.
2. Double-click ${SETTINGS_SHORTCUT_NAME} to jump to System Settings -> Privacy & Security.
3. In Privacy & Security, click "Open Anyway" for ${APP_NAME}.
4. Go back to /Applications, right-click ${APP_NAME}.app, choose Open once, then confirm.

For Codex visible control:
- Open System Settings -> Privacy & Security -> Accessibility.
- Enable ${APP_NAME}. If it was already enabled after an update, toggle it off and on once.
EOF

hdiutil detach "${MOUNT_DIR}" -quiet
hdiutil convert "${RW_DMG}" -format UDZO -imagekey zlib-level=9 -o "${FINAL_DMG}" -quiet
mv "${FINAL_DMG}" "${DMG_PATH}"
