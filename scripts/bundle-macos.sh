#!/usr/bin/env bash
# Builds dist/Quran.app (and dist/Quran.dmg with DMG=1) on macOS.
#
#   scripts/bundle-macos.sh              # native architecture, ad-hoc signature
#   UNIVERSAL=1 scripts/bundle-macos.sh  # arm64 + x86_64 universal binary
#   DMG=1 scripts/bundle-macos.sh        # also a disk image with an Applications link
#   SIGN_IDENTITY="Developer ID Application: …" scripts/bundle-macos.sh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "bundle-macos.sh must run on macOS (needs sips, iconutil, lipo, codesign)" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="dist/Quran.app"
ENTITLEMENTS="packaging/macos/entitlements.plist"

# The window code calls NSWindow.toolbarStyle, new in macOS 11; this also reaches the C code
# that cc builds (SQLite, zstd).
export MACOSX_DEPLOYMENT_TARGET=11.0

if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release -p quran-rs --target aarch64-apple-darwin --target x86_64-apple-darwin
  BIN="$(mktemp -d)/quran-rs"
  lipo -create -output "$BIN" \
    target/aarch64-apple-darwin/release/quran-rs \
    target/x86_64-apple-darwin/release/quran-rs
else
  cargo build --release -p quran-rs
  BIN="target/release/quran-rs"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Quran"

# App icon from the 1024 px PNG (crates/app/examples/make_icon.rs draws it).
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
SRC="crates/app/assets/icon.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Quran</string>
  <key>CFBundleDisplayName</key><string>Quran</string>
  <key>CFBundleIdentifier</key><string>dev.quran-rs.app</string>
  <key>CFBundleExecutable</key><string>Quran</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>${MACOSX_DEPLOYMENT_TARGET}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.reference</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
  <key>NSHumanReadableCopyright</key><string>MIT License. Quran text and translations © Tanzil Project (tanzil.net).</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  SIGN=(codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY")
else
  SIGN=(codesign --force --options runtime --sign -)
fi
"${SIGN[@]}" --entitlements "$ENTITLEMENTS" "$APP"
echo "Built $APP ($VERSION, $(lipo -archs "$APP/Contents/MacOS/Quran"))"

if [[ "${DMG:-0}" == "1" ]]; then
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f dist/Quran.dmg
  hdiutil create -volname Quran -srcfolder "$STAGE" -ov -format UDZO dist/Quran.dmg >/dev/null
  echo "Built dist/Quran.dmg"
fi
