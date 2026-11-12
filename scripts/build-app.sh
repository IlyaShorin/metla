#!/usr/bin/env bash
# Builds Metla.app: the core (scripts/build-core.sh), the Swift package, and
# the bundle layout with its Info.plist, signed ad-hoc, or with a
# certificate and a secure timestamp when METLA_SIGN_IDENTITY names one
# (either way no Developer ID, no hardened runtime, not notarised).
# Prints the bundle's path.
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
#   METLA_SIGN_IDENTITY   a signing identity of your keychain, passed to
#                         codesign as given: the bundle is signed with its
#                         certificate and a secure timestamp from Apple's
#                         server (codesign --timestamp), so the signature
#                         stays valid after the certificate expires. The
#                         timestamp needs the network: without an answer
#                         from the server the build fails. macOS ties a
#                         Full Disk Access grant to the signature; an
#                         ad-hoc one changes with every build, a
#                         certificate's does not, so the grant survives
#                         updates. Published releases after 0.7.2 are
#                         signed this way by the maintainer; a fork, or
#                         your own install, uses your own identity. The
#                         signature shows the certificate's name to anyone
#                         who inspects the app. The script never prints
#                         the identity, and passes on none of codesign's
#                         lines that could name it. Without the variable
#                         the bundle is signed ad-hoc (an ad-hoc signature
#                         cannot be timestamped). A release build makes
#                         Metla.zip either way

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
version="0.7.3"

lib="$("$repo/scripts/build-core.sh" "$flavour")"
# Each flavour links in a build directory of its own, so a plain app can
# never be left linked with the production library (or the other way).
scratch="$build/swift-build"
[[ "$name" == release-plain ]] && scratch="$build/swift-build-plain"
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
products="$(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" \
    "${swift_flags[@]}" --show-bin-path)"
# SwiftPM does not know the core library as an input of the link: when no
# Swift source changed it would not link again, and the bundle would get
# the old core. The executable goes first, so it is always linked anew.
rm -f -- "$products/Metla"
(cd "$repo/app" && swift build -c "$configuration" --scratch-path "$scratch" "${swift_flags[@]}") >&2
stamp="$(cat "$lib/stamp")"
if ! LC_ALL=C grep -q -a -F "$stamp" "$products/Metla"; then
    echo "the app does not contain the core just built (stamp $stamp): not bundled" >&2
    exit 1
fi

app="$build/$name/Metla.app"
# The old zip goes first: a build that fails later leaves none beside the
# new bundle.
rm -f -- "$build/$name/Metla.zip"
rm -rf -- "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$products/Metla" "$app/Contents/MacOS/Metla"
cp -R "$products/Metla_MetlaKit.bundle" "$app/Contents/Resources/"
# The Dock and Finder icon, drawn by scripts/make-icon.sh and committed.
cp "$repo/app/Icon/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleLocalizations</key><array><string>en</string><string>ru</string></array>
    <key>CFBundleExecutable</key><string>Metla</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIdentifier</key><string>org.metla.Metla</string>
    <key>CFBundleName</key><string>Metla</string>
    <key>CFBundleDisplayName</key><string>Metla</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>10</string>
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
identity="${METLA_SIGN_IDENTITY:-}"
if [[ -n "$identity" ]]; then
    # codesign's messages can name the certificate: the identity as given,
    # or the certificates it matched, in quotes. After a success none is
    # shown; after a failure, only the lines that hold neither.
    if ! said="$(codesign --force --timestamp --sign "$identity" "$app" 2>&1)"; then
        printf '%s\n' "$said" | LC_ALL=C grep -v -F -e "$identity" -e '"' >&2 || true
        echo "codesign failed with METLA_SIGN_IDENTITY (for example: no identity by that name or more than one, a locked keychain, no answer from the timestamp server): not signed, no Metla.zip" >&2
        exit 1
    fi
    echo "signed with METLA_SIGN_IDENTITY's certificate and a secure timestamp" >&2
else
    codesign --force --sign - "$app" >&2
    echo "signed ad-hoc" >&2
fi
if [[ "$configuration" == release ]]; then
    (cd "$build/$name" && ditto -c -k --keepParent Metla.app Metla.zip)
fi
echo "$app"
