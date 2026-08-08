#!/usr/bin/env bash
# Build the release binaries cargo-packager bundles. Invoked as the packager's
# before-packaging-command, not run by hand.
#
# macOS: build both arches and lipo them into universal binaries at
#        target/release/, so the shipped .app runs on Apple Silicon and Intel.
# Linux: a plain native release build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BINS=(kairn-app kairn)

if [[ "$(uname)" == "Darwin" ]]; then
    TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
    for t in "${TARGETS[@]}"; do
        rustup target add "$t" >/dev/null 2>&1 || true
        cargo build --release --target "$t" -p kairn-app -p kairn-cli
    done
    mkdir -p target/release
    for b in "${BINS[@]}"; do
        lipo -create -output "target/release/$b" \
            "target/aarch64-apple-darwin/release/$b" \
            "target/x86_64-apple-darwin/release/$b"
        lipo -info "target/release/$b"
    done
else
    cargo build --release -p kairn-app -p kairn-cli
fi
