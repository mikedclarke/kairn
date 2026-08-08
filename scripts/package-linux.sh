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

echo "==> done"
ls -la "$ROOT/dist" | grep -E '\.(deb|AppImage)$' || true
