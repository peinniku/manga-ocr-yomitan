#!/usr/bin/env bash
# Build pic_yomi.app (release) and, with --install, copy it to /Applications.
#
#   scripts/bundle-macos.sh            # -> target/release/pic_yomi.app
#   scripts/bundle-macos.sh --install  # also copies to /Applications
#   scripts/bundle-macos.sh --debug    # faster debug build for development
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=release
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=1 ;;
    --debug) PROFILE=debug ;;
    *) echo "unknown option $arg" >&2; exit 2 ;;
  esac
done

if [ "$PROFILE" = release ]; then
  cargo build --release -p picyomi
else
  cargo build -p picyomi
fi

# Phone web app (served by the desktop app at /app).
if [ -f web/package.json ]; then
  (cd web && npm ci --silent && npm run build --silent)
fi

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP="target/$PROFILE/pic_yomi.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/picyomi" "$APP/Contents/MacOS/picyomi"
if [ -d web/dist ]; then
  cp -R web/dist "$APP/Contents/Resources/web"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>pic_yomi</string>
  <key>CFBundleDisplayName</key><string>pic_yomi</string>
  <key>CFBundleIdentifier</key><string>local.picyomi.app</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>picyomi</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSScreenCaptureUsageDescription</key><string>pic_yomi 识别鼠标附近屏幕上的日语文字，用于查词。截图只在本机处理。</string>
</dict>
</plist>
PLIST

# Sign with a stable local identity when there is one, so macOS keeps the
# Screen Recording permission across rebuilds; otherwise ad-hoc. Create it with
# scripts/make-signing-cert.sh.
IDENTITY="pic_yomi Local Signing"
if security find-certificate -c "$IDENTITY" >/dev/null 2>&1; then
    codesign --force --deep --sign "$IDENTITY" "$APP" >/dev/null
else
    echo "note: no \"$IDENTITY\" certificate; signing ad-hoc (Screen Recording must be re-granted after each build)"
    codesign --force --deep --sign - "$APP" >/dev/null
fi
echo "built $APP"

if [ "$INSTALL" = 1 ]; then
  rm -rf "/Applications/pic_yomi.app"
  cp -R "$APP" /Applications/
  echo "installed /Applications/pic_yomi.app"
fi
