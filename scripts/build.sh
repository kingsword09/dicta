#!/usr/bin/env bash
# Build vo in release mode, ad-hoc codesign with entitlements + Info.plist embed.
# Requires: macOS 26, Xcode 26 toolchain, Apple Silicon.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ENTITLEMENTS="$ROOT/Resources/vo.entitlements"
INFO_PLIST="$ROOT/Resources/Info.plist"

# 1. Build with Info.plist embedded into __TEXT,__info_plist (required for TCC on bare binary).
echo ">> swift build -c release (with embedded Info.plist)"
swift build -c release --arch arm64 \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$INFO_PLIST"

BIN_PATH="$(swift build -c release --arch arm64 --show-bin-path)/vo"

# 2. Ad-hoc sign with entitlements + hardened runtime.
echo ">> ad-hoc codesign"
/usr/bin/codesign \
    --force \
    --sign - \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$BIN_PATH"

echo ">> built: $BIN_PATH"
echo ">> verify with: codesign -dv --entitlements - $BIN_PATH"
