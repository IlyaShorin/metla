#!/usr/bin/env bash
# Draws Metla's app icon at every size of a macOS icon set with
# scripts/make-icon.swift (shapes only: each size is drawn at its own
# pixel size, nothing is scaled), joins them with iconutil and writes
# app/Icon/AppIcon.icns, which scripts/build-app.sh puts in the bundle.
# The icns is committed, so a build does not draw it; run this after
# changing the drawing.
#
#   scripts/make-icon.sh

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/metla-icon.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

swiftc -O -o "$work/make-icon" "$repo/scripts/make-icon.swift"
set="$work/AppIcon.iconset"
mkdir "$set"
for points in 16 32 128 256 512; do
    "$work/make-icon" "$set/icon_${points}x${points}.png" "$points"
    "$work/make-icon" "$set/icon_${points}x${points}@2x.png" "$((points * 2))"
done
mkdir -p "$repo/app/Icon"
iconutil -c icns -o "$repo/app/Icon/AppIcon.icns" "$set"
echo "$repo/app/Icon/AppIcon.icns"
