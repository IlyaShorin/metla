#!/usr/bin/env bash
# Builds Metla.app: the core (scripts/build-core.sh), the Swift package in
# release (METLA_CONFIGURATION=debug for a debug build), and the bundle
# layout with its Info.plist, ad-hoc signed. Prints the bundle's path.
#
# Only the release build links the core built with its production
# capability (scripts/build-core.sh production) and can clean a root that
# is not a fixture; the debug build cleans fixtures only. Start a release
# build on a fixture (METLA_ROOT) while developing.
#
# Environment:
#   METLA_BUILD           where the bundle and SwiftPM's build directory go
#                         (default target/app in the repository: keep it off
#                         a full disk)
#   METLA_CONFIGURATION   release (default) or debug

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${METLA_BUILD:-$repo/target/app}"
configuration="${METLA_CONFIGURATION:-release}"
version="0.5.0"

case "$configuration" in
    release) kind=production ;;
    debug) kind=plain ;;
    *) echo "METLA_CONFIGURATION must be release or debug" >&2; exit 2 ;;
esac
lib="$("$repo/scripts/build-core.sh" "$kind")"
scratch="$build/swift-build"
(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" \
    -Xswiftc -warnings-as-errors -Xlinker -L"$lib") >&2
products="$(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" \
    -Xlinker -L"$lib" --show-bin-path)"

app="$build/$configuration/Metla.app"
rm -rf -- "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$products/Metla" "$app/Contents/MacOS/Metla"
cp -R "$products/Metla_MetlaKit.bundle" "$app/Contents/Resources/"
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleLocalizations</key><array><string>en</string><string>ru</string></array>
    <key>CFBundleExecutable</key><string>Metla</string>
    <key>CFBundleIdentifier</key><string>org.metla.Metla</string>
    <key>CFBundleName</key><string>Metla</string>
    <key>CFBundleDisplayName</key><string>Metla</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>5</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><false/>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSHumanReadableCopyright</key><string>Metla contributors, MIT License</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app" >&2
echo "$app"
