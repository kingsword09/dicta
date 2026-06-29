#!/usr/bin/env bash
# Build the Apple Speech native adapter in release mode.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ADAPTER="$ROOT/adapters/apple-speech"
cd "$ADAPTER"

ENTITLEMENTS="$ADAPTER/Resources/vo.entitlements"
INFO_PLIST="$ADAPTER/Resources/Info.plist"

echo ">> swift build -c release (with embedded Info.plist)"
swift build -c release --arch arm64 \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$INFO_PLIST"

BIN_PATH="$(swift build -c release --arch arm64 --show-bin-path)/vo"

SIGN_IDENTITY="${VO_CODESIGN_IDENTITY:--}"
echo ">> codesign (identity: $SIGN_IDENTITY)"
/usr/bin/codesign \
    --force \
    --sign "$SIGN_IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$BIN_PATH"

echo ">> built: $BIN_PATH"
