#!/usr/bin/env bash
# Builds Metla.app: the core (scripts/build-core.sh), the Swift package, and
# the bundle layout with its Info.plist, ad-hoc signed (no hardened runtime,
# not notarised). Prints the bundle's path.
#
#   scripts/build-app.sh [--release]    release Swift linked with the core's
#                                       production library: the only build
#                                       that can clean a root that is not a
#                                       fixture. Symbols stripped, no local
#                                       path inside a binary (checked: the
#                                       build fails otherwise), and Metla.zip
#                                       beside the bundle. <build>/release
#   scripts/build-app.sh --release --plain
#                                       the same release build linked with
#                                       the plain library: it cleans
#                                       fixtures only, so it may be started
#                                       on the real disk to measure it.
#                                       <build>/release-plain
#   scripts/build-app.sh --debug        debug Swift, plain library: develop,
#                                       test and capture with it. <build>/debug
#
# Start a production build only with METLA_ROOT on a fixture while
# developing (scripts/run-app.sh).
#
# Environment:
#   METLA_BUILD           where the bundle and SwiftPM's build directory go
#                         (default target/app in the repository: keep it off
#                         a full disk)
#   METLA_CONFIGURATION   release or debug, as --release / --debug

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd -P)"
configuration="${METLA_CONFIGURATION:-release}"
flavour=production
for argument in "$@"; do
    case "$argument" in
        --release) configuration=release ;;
        --debug) configuration=debug ;;
        --plain) flavour=plain ;;
        *) echo "usage: $0 [--release [--plain] | --debug]" >&2; exit 2 ;;
    esac
done
case "$configuration" in
    release) ;;
    debug) flavour=plain ;;
    *) echo "METLA_CONFIGURATION must be release or debug" >&2; exit 2 ;;
esac
name="$configuration"
[[ "$configuration" == release && "$flavour" == plain ]] && name=release-plain

mkdir -p "${METLA_BUILD:-$repo/target/app}"
build="$(cd "${METLA_BUILD:-$repo/target/app}" && pwd -P)"
version="0.7.0"

lib="$("$repo/scripts/build-core.sh" "$flavour")"
scratch="$build/swift-build"
# No local path in a release binary: no debug information (the symbols
# are stripped anyway), and Swift's file paths mapped to neutral names, as
# build-core.sh maps Rust's.
swift_flags=(-Xswiftc -warnings-as-errors -Xlinker -L"$lib")
if [[ "$configuration" == release ]]; then
    swift_flags+=(-Xswiftc -gnone)
    for map in "$repo=/metla" "$build=/build" "$HOME=/home"; do
        swift_flags+=(-Xswiftc -file-prefix-map -Xswiftc "$map")
    done
fi
(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" "${swift_flags[@]}") >&2
products="$(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" \
    "${swift_flags[@]}" --show-bin-path)"

app="$build/$name/Metla.app"
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
    <key>CFBundleVersion</key><string>7</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><false/>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSHumanReadableCopyright</key><string>Metla contributors, MIT License</string>
</dict>
</plist>
PLIST

if [[ "$configuration" == release ]]; then
    strip -x "$app/Contents/MacOS/Metla"
    "$repo/scripts/no-local-paths.sh" "$app" "$build" >&2
fi
codesign --force --sign - "$app" >&2
if [[ "$configuration" == release ]]; then
    rm -f -- "$build/$name/Metla.zip"
    (cd "$build/$name" && ditto -c -k --keepParent Metla.app Metla.zip)
fi
echo "$app"
