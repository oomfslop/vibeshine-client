#!/usr/bin/env bash
# Builds sloplight in release mode and assembles Sloplight.app.
# The bundle declares the Local Network / Bonjour usage macOS requires for
# LAN connections and mDNS discovery, and embeds the streaming helper
# (the Moonlight.app from `nix build '.?submodules=1'`) when it exists.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
APP=target/Sloplight.app
[ -e "$APP" ] && chmod -R u+w "$APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Helpers"
cp target/release/sloplight "$APP/Contents/MacOS/sloplight"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Sloplight</string>
  <key>CFBundleDisplayName</key><string>Sloplight</string>
  <key>CFBundleIdentifier</key><string>com.sloplight.launcher</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>sloplight</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Sloplight finds and talks to Apollo, Sunshine and GeForce Experience hosts on your local network.</string>
  <key>NSBonjourServices</key>
  <array><string>_nvstream._tcp</string></array>
</dict>
</plist>
PLIST
HELPER_SRC=../result/Applications/Moonlight.app
if [ -d "$HELPER_SRC" ]; then
  # Dereference the Nix store symlinks so the bundle is self-contained
  cp -RL "$HELPER_SRC" "$APP/Contents/Helpers/Moonlight.app"
  chmod -R u+w "$APP/Contents/Helpers/Moonlight.app"  # copies from the Nix store are read-only
  # Give the helper its own identity so Launch Services doesn't apply the installed
  # Moonlight.app's per-app settings (notably the notch "safe area" compatibility mode).
  HELPER_PLIST="$APP/Contents/Helpers/Moonlight.app/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.sloplight.helper" "$HELPER_PLIST"
  /usr/libexec/PlistBuddy -c "Delete :NSPrefersDisplaySafeAreaCompatibilityMode" "$HELPER_PLIST" 2>/dev/null || true
  # false = full-screen Space sits below the camera housing unscaled (true only makes macOS
  # scale the whole output down to fit below it).
  /usr/libexec/PlistBuddy -c "Add :NSPrefersDisplaySafeAreaCompatibilityMode bool false" "$HELPER_PLIST"
  echo "Embedded streaming helper from $HELPER_SRC"
else
  echo "No helper at $HELPER_SRC (run nix build '.?submodules=1' in moonlight-qt first); the app will look for Moonlight.app elsewhere"
fi
# Ad-hoc signature so macOS attributes the Local Network permission to this bundle
codesign --force --deep --sign - "$APP" 2>/dev/null || true
# Register both bundles so Launch Services picks up the identifiers and plist keys.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
"$LSREGISTER" -f "$APP/Contents/Helpers/Moonlight.app" >/dev/null 2>&1 || true
"$LSREGISTER" -f "$APP" >/dev/null 2>&1 || true
echo "Built $APP"
echo "Run:  open $APP      (macOS asks for Local Network access on first use)"
