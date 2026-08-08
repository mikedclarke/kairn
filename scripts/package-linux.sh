#!/usr/bin/env bash
# Build the Linux distributables: a .deb and an .AppImage. Run this on a Linux
# machine (no cross-compiling from macOS). No signing is involved.
#
#   scripts/package-linux.sh
#
# The GPUI build needs the dev headers listed in the README (fontconfig,
# Wayland/X11, Vulkan, ALSA) plus a working Rust toolchain.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# linuxdeploy (used for the AppImage) bundles an old `strip` that can't parse
# modern glibc's `.relr.dyn` ELF sections, so it aborts on Fedora 44+ system
# libraries (libxkbcommon, libXau, ...). Skip stripping; the release binary is
# already built without debug bloat. Override with NO_STRIP=false if needed.
export NO_STRIP="${NO_STRIP:-true}"

cargo packager --release -p kairn-app -f deb,appimage --out-dir "$ROOT/dist" -v

# cargo-packager names both files after the main binary ("kairn-app") and gives
# the AppImage no filename override, so normalise them to the product name.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
deb="$(ls "$ROOT"/dist/*_amd64.deb 2>/dev/null | head -1)"
if [ -n "$deb" ] && [ "$(basename "$deb")" != "kairn_${VERSION}_amd64.deb" ]; then
    mv "$deb" "$ROOT/dist/kairn_${VERSION}_amd64.deb"
fi
app="$(ls "$ROOT"/dist/*_x86_64.AppImage 2>/dev/null | head -1)"
if [ -n "$app" ] && [ "$(basename "$app")" != "Kairn-${VERSION}-x86_64.AppImage" ]; then
    mv "$app" "$ROOT/dist/Kairn-${VERSION}-x86_64.AppImage"
fi

echo "==> done"
ls -la "$ROOT/dist" | grep -E '\.(deb|AppImage)$' || true
