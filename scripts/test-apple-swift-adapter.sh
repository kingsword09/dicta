#!/usr/bin/env bash
# Run the isolated Apple Swift adapter tests when a macOS 26 runtime is available.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ADAPTER="$ROOT/legacy/apple-swift-adapter"
cd "$ADAPTER"

OS_MAJOR="$(sw_vers -productVersion | cut -d. -f1)"
if [ "$OS_MAJOR" -lt 26 ]; then
    echo ">> skipping Apple Swift adapter tests: macOS 26 runtime required, found $(sw_vers -productVersion)"
    exit 0
fi

DEV="$(xcode-select -p)"
CLT_FRAMEWORKS="$DEV/Library/Developer/Frameworks"
CLT_INTEROP="$DEV/Library/Developer/usr/lib"

EXTRA=()
if [ -d "$CLT_FRAMEWORKS/Testing.framework" ]; then
    EXTRA=(
        -Xswiftc -F -Xswiftc "$CLT_FRAMEWORKS"
        -Xlinker -rpath -Xlinker "$CLT_FRAMEWORKS"
        -Xlinker -rpath -Xlinker "$CLT_INTEROP"
    )
fi

exec swift test ${EXTRA[@]+"${EXTRA[@]}"} "$@"
