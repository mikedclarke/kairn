#!/usr/bin/env bash
# Build, sign, notarize, and staple a distributable macOS Kairn.app + .dmg.
#
# Maintainer script: it needs a "Developer ID Application" certificate and a
# notarytool keychain profile. If you fork Kairn, override SIGN_IDENTITY and
# NOTARY_PROFILE with your own, or set NOTARIZE=0 SIGN=0 to build unsigned.
#
#   scripts/package-macos.sh
#   NOTARIZE=0 scripts/package-macos.sh      # sign but skip notarization
#   SIGN=0 NOTARIZE=0 scripts/package-macos.sh   # bundle only, no signing
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SIGN="${SIGN:-1}"
NOTARIZE="${NOTARIZE:-1}"
SIGN_IDENTITY="${SIGN_IDENTITY:-Developer ID Application: Michael Clarke (7R85K5967B)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-KairnNotary}"
# Absolute so cargo-packager writes to the workspace root, not the crate dir
# (--out-dir is resolved relative to the packaged crate's Cargo.toml).
OUT_DIR="${OUT_DIR:-$ROOT/dist}"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml"; exit 1; }

echo "==> packaging Kairn $VERSION (universal .app)"
cargo packager --release -p kairn-app -f app --out-dir "$OUT_DIR" -v

APP="$(find "$OUT_DIR" -maxdepth 2 -name 'Kairn.app' -type d | head -1)"
[ -n "$APP" ] || { echo "Kairn.app not found under $OUT_DIR"; exit 1; }
echo "==> bundle: $APP"

if [ "$SIGN" = "1" ]; then
    echo "==> signing (hardened runtime)"
    # Sign helper binaries (e.g. the bundled `kairn` CLI) inside-out first, then
    # seal the whole bundle — which signs the main `kairn-app` executable. Signing
    # the main executable on its own makes codesign treat it as the bundle main
    # and reject the still-unsigned siblings.
    while IFS= read -r f; do
        [ "$(basename "$f")" = "kairn-app" ] && continue
        codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$f"
    done < <(find "$APP/Contents/MacOS" -type f)
    codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
    codesign --verify --deep --strict --verbose=2 "$APP"
    codesign -dvvv "$APP" 2>&1 | grep -E 'Authority|flags' | head -3
fi

if [ "$NOTARIZE" = "1" ]; then
    echo "==> notarizing app"
    ZIP="$OUT_DIR/Kairn-app.zip"
    rm -f "$ZIP"
    ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP"
    rm -f "$ZIP"
fi

echo "==> building dmg"
DMG="$OUT_DIR/Kairn-$VERSION.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "Kairn" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

if [ "$SIGN" = "1" ]; then
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
fi
if [ "$NOTARIZE" = "1" ]; then
    echo "==> notarizing dmg"
    xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
fi

echo "==> done"
echo "    app: $APP"
echo "    dmg: $DMG"
