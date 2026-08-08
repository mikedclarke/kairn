#!/usr/bin/env bash
# Build the KairnFFI XCFramework: kairn-core + kairn-sync behind one UniFFI
# facade, compiled for iOS device and simulator, plus the generated Swift
# bindings. The kairn-mobile app consumes the output as a prebuilt artifact, so
# day-to-day Swift work there never needs the Rust toolchain — only this script
# does, and only when the Rust core changes.
#
#   scripts/build-ios-framework.sh                 # release build into dist/ios
#   PROFILE=debug scripts/build-ios-framework.sh   # faster, unoptimised
#   OUT_DIR=../kairn-mobile/KairnFFI/vendor scripts/build-ios-framework.sh
#
# Prereqs: the iOS Rust targets and Xcode.
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CRATE=kairn-ffi
LIB=libkairn_ffi.a          # cargo staticlib output (crate name, - -> _)
MODULE=kairn_ffi            # uniffi namespace; Swift C module is ${MODULE}FFI
PROFILE="${PROFILE:-release}"
DEVICE_TARGET=aarch64-apple-ios
SIM_TARGET=aarch64-apple-ios-sim
OUT_DIR="${OUT_DIR:-$ROOT/dist/ios}"

BUILD_FLAG=--release
[ "$PROFILE" = debug ] && BUILD_FLAG=

for t in "$DEVICE_TARGET" "$SIM_TARGET"; do
    rustup target list --installed | grep -qx "$t" || {
        echo "missing rust target $t — run: rustup target add $t" >&2
        exit 1
    }
done

echo "==> building $CRATE for $DEVICE_TARGET and $SIM_TARGET ($PROFILE)"
cargo build -p "$CRATE" --target "$DEVICE_TARGET" $BUILD_FLAG
cargo build -p "$CRATE" --target "$SIM_TARGET" $BUILD_FLAG

DEVICE_LIB="$ROOT/target/$DEVICE_TARGET/$PROFILE/$LIB"
SIM_LIB="$ROOT/target/$SIM_TARGET/$PROFILE/$LIB"

echo "==> generating Swift bindings (library mode)"
GEN="$ROOT/target/uniffi-gen"
rm -rf "$GEN"; mkdir -p "$GEN"
cargo run -p "$CRATE" --bin uniffi-bindgen -- generate \
    --library "$DEVICE_LIB" --language swift --out-dir "$GEN"
# uniffi emits: $MODULE.swift, ${MODULE}FFI.h, ${MODULE}FFI.modulemap

echo "==> assembling headers"
HEADERS="$GEN/headers"
rm -rf "$HEADERS"; mkdir -p "$HEADERS"
cp "$GEN/${MODULE}FFI.h" "$HEADERS/"
# An xcframework's static-library slice wants its module map named
# module.modulemap alongside the header.
cp "$GEN/${MODULE}FFI.modulemap" "$HEADERS/module.modulemap"

echo "==> assembling XCFramework"
XCF="$OUT_DIR/KairnFFI.xcframework"
rm -rf "$XCF"; mkdir -p "$OUT_DIR"
xcodebuild -create-xcframework \
    -library "$DEVICE_LIB" -headers "$HEADERS" \
    -library "$SIM_LIB" -headers "$HEADERS" \
    -output "$XCF"

cp "$GEN/$MODULE.swift" "$OUT_DIR/$MODULE.swift"

echo "==> done"
echo "    framework: $XCF"
echo "    bindings:  $OUT_DIR/$MODULE.swift"
du -sh "$XCF"
