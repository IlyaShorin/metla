#!/usr/bin/env bash
# Builds the Rust core for the app and prints the directory that holds
# libmetla_ffi.a, which the Swift build passes to the linker.
#
#   scripts/build-core.sh              fixtures only: the tests and the
#                                      debug app (target/app-core/plain)
#   scripts/build-core.sh production   the app's release build: the only
#                                      library that can clean a root that is
#                                      not a fixture (target/app-core/production)
#
#   * metla-ffi's static library, release, for aarch64-apple-darwin, and
#     for x86_64-apple-darwin too when that Rust target is installed (no
#     toolchain is downloaded), joined into one library with lipo;
#   * the Swift bindings and the C header, generated from that library by
#     uniffi-bindgen-swift, and a module map that links the library and
#     what it needs, into app/Sources/MetlaCoreFFI/generated (ignored by
#     Git).
#
# Build output goes to core/target (on the development machine a link to
# a RAM disk: keep big output off a full disk).
#
# No local path goes into the library: the home directory, Cargo's and
# rustup's homes and the repository are mapped to /home, /cargo, /rustup
# and /metla in every crate's file names (panic locations); rustc applies
# the last prefix that matches. scripts/no-local-paths.sh checks the app.

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
core="$repo/core"
generated="$repo/app/Sources/MetlaCoreFFI/generated"
kind="${1:-plain}"
case "$kind" in
    plain) features=() ;;
    production) features=(--features production) ;;
    *) echo "usage: $0 [plain|production]" >&2; exit 2 ;;
esac
out="$core/target/app-core"
lib_dir="$out/$kind/lib"
bindings="$out/bindings"

triples=(aarch64-apple-darwin)
if rustup target list --installed 2>/dev/null | grep -qx x86_64-apple-darwin; then
    triples+=(x86_64-apple-darwin)
fi

# Encoded (one flag per field) because the paths may hold spaces.
separator=$'\x1f'
remap=""
for map in "$HOME=/home" "${CARGO_HOME:-$HOME/.cargo}=/cargo" "${RUSTUP_HOME:-$HOME/.rustup}=/rustup" \
    "$repo=/metla" "$(cd "$repo" && pwd -P)=/metla"; do
    remap="${remap:+$remap$separator}--remap-path-prefix=$map"
done

# The core's stamp: a hash of its sources (paths relative to core/ and
# contents) and the kind, compiled into the library (metla_ffi's
# core_stamp) and written beside it, so build-app.sh can check that the
# app links the core it just built. No path and no time go into it: the
# same sources give the same stamp.
stamp="$(cd "$core" && { find . -path ./target -prune -o \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -type f -print | LC_ALL=C sort | while IFS= read -r file; do printf '%s\n' "$file"; cat "$file"; done; } | shasum -a 256 | cut -c1-16)-$kind"

libraries=()
for triple in "${triples[@]}"; do
    (cd "$core" && METLA_CORE_STAMP="$stamp" CARGO_ENCODED_RUSTFLAGS="$remap" cargo rustc -q -p metla-ffi --lib --release \
        --target "$triple" ${features[@]+"${features[@]}"} --crate-type staticlib) >&2
    libraries+=("$core/target/$triple/release/libmetla_ffi.a")
done

# Both kinds build to the same file per triple: join them at once.
mkdir -p "$lib_dir" "$bindings"
lipo -create "${libraries[@]}" -output "$lib_dir/libmetla_ffi.a"
printf '%s\n' "$stamp" >"$lib_dir/stamp"

(cd "$core" && cargo run -q -p metla-ffi --features bindgen --bin uniffi-bindgen-swift -- \
    --swift-sources --headers --metadata-no-deps \
    "$lib_dir/libmetla_ffi.a" "$bindings") >&2

mkdir -p "$generated/include"
# Copied only when they differ: an unchanged interface leaves the Swift
# build alone (build-app.sh links the app anew in any case).
cmp -s "$bindings/metla_ffi.swift" "$generated/metla_ffi.swift" \
    || cp "$bindings/metla_ffi.swift" "$generated/metla_ffi.swift"
cmp -s "$bindings/metla_ffiFFI.h" "$generated/include/metla_ffiFFI.h" \
    || cp "$bindings/metla_ffiFFI.h" "$generated/include/metla_ffiFFI.h"
# What the static library needs at link time, as
# `cargo rustc ... -- --print native-static-libs` lists it.
cat >"$generated/include/module.modulemap" <<'MAP'
module metla_ffiFFI {
    header "metla_ffiFFI.h"
    link "metla_ffi"
    link "sqlite3"
    link "iconv"
    link framework "CoreServices"
    link framework "CoreFoundation"
    export *
}
MAP

echo "$lib_dir"
