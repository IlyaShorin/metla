#!/usr/bin/env bash
# Builds the Rust core for the app and prints the directory that holds
# libmetla_ffi.a, which the Swift build passes to the linker.
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

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
core="$repo/core"
generated="$repo/app/Sources/MetlaCoreFFI/generated"
out="$core/target/app-core"
bindings="$out/bindings"

triples=(aarch64-apple-darwin)
if rustup target list --installed 2>/dev/null | grep -qx x86_64-apple-darwin; then
    triples+=(x86_64-apple-darwin)
fi

libraries=()
for triple in "${triples[@]}"; do
    (cd "$core" && cargo rustc -q -p metla-ffi --lib --release --target "$triple" \
        --crate-type staticlib) >&2
    libraries+=("$core/target/$triple/release/libmetla_ffi.a")
done

mkdir -p "$out/lib" "$bindings"
lipo -create "${libraries[@]}" -output "$out/lib/libmetla_ffi.a"

(cd "$core" && cargo run -q -p metla-ffi --features bindgen --bin uniffi-bindgen-swift -- \
    --swift-sources --headers --metadata-no-deps \
    "$out/lib/libmetla_ffi.a" "$bindings") >&2

mkdir -p "$generated/include"
cp "$bindings/metla_ffi.swift" "$generated/metla_ffi.swift"
cp "$bindings/metla_ffiFFI.h" "$generated/include/metla_ffiFFI.h"
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

echo "$out/lib"
