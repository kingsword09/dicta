#!/usr/bin/env bash
# Build vo in release mode, codesign with entitlements + Info.plist embed.
# Signs ad-hoc by default; set VO_CODESIGN_IDENTITY for a stable signature.
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

# 2. Sign with entitlements + hardened runtime. Defaults to ad-hoc; set
# VO_CODESIGN_IDENTITY to a stable identity (Developer ID, or a self-signed
# code-signing cert) so the signature, hence TCC attribution, survives rebuilds.
SIGN_IDENTITY="${VO_CODESIGN_IDENTITY:--}"
echo ">> codesign (identity: $SIGN_IDENTITY)"
/usr/bin/codesign \
    --force \
    --sign "$SIGN_IDENTITY" \
    --entitlements "$ENTITLEMENTS" \
    --options runtime \
    "$BIN_PATH"

echo ">> built: $BIN_PATH"
echo ">> verify with: codesign -dv --entitlements - $BIN_PATH"

if [ "$SIGN_IDENTITY" = "-" ]; then
    echo ">> NOTE: ad-hoc signed. The cdhash changes every rebuild, so macOS"
    echo ">>       re-prompts for TCC after each build and grants do not persist."
    echo ">>       Set VO_CODESIGN_IDENTITY to a stable identity for persistent"
    echo ">>       permissions."
fi
