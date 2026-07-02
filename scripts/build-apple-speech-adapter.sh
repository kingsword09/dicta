#!/usr/bin/env bash
# Build the Apple Speech native adapter in release mode.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ADAPTER="$ROOT/adapters/apple-speech"
cd "$ADAPTER"

ENTITLEMENTS="$ADAPTER/Resources/dicta.entitlements"
INFO_PLIST="$ADAPTER/Resources/Info.plist"

echo ">> swift build -c release (with embedded Info.plist)"
swift build -c release --arch arm64 \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$INFO_PLIST"

BIN_DIR="$(swift build -c release --arch arm64 --show-bin-path)"
SWIFT_BIN_PATH="$BIN_DIR/dicta"
BIN_PATH="$BIN_DIR/dicta-adapter-apple-speech"

SIGN_IDENTITY="${DICTA_CODESIGN_IDENTITY:--}"
echo ">> codesign (identity: $SIGN_IDENTITY)"
/usr/bin/codesign \
    --force \
    --sign "$SIGN_IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$SWIFT_BIN_PATH"

cp "$SWIFT_BIN_PATH" "$BIN_PATH"

echo ">> built: $BIN_PATH"
