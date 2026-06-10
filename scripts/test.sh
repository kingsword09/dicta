#!/usr/bin/env bash
# Run the Swift Testing suite via SwiftPM.
#
# Under a full Xcode toolchain (e.g. GitHub's macos runners) `swift test` resolves
# the Swift Testing framework on its own, so no extra flags are needed. Under a
# Command Line Tools-only install the Testing.framework and its lib_TestingInterop
# dylib are not on the default search / rpath, so we wire them up explicitly.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DEV="$(xcode-select -p)"
CLT_FRAMEWORKS="$DEV/Library/Developer/Frameworks"
CLT_INTEROP="$DEV/Library/Developer/usr/lib"

EXTRA=()
if [ -d "$CLT_FRAMEWORKS/Testing.framework" ]; then
    # Command Line Tools layout: point the compiler at the framework and add both
    # the framework dir and the interop dylib dir to the runtime search path.
    EXTRA=(
        -Xswiftc -F -Xswiftc "$CLT_FRAMEWORKS"
        -Xlinker -rpath -Xlinker "$CLT_FRAMEWORKS"
        -Xlinker -rpath -Xlinker "$CLT_INTEROP"
    )
fi

# `${EXTRA[@]+...}` so an empty array does not trip `set -u` (older bash treats an
# empty array reference as an unbound variable, which is what CI's bash does).
exec swift test ${EXTRA[@]+"${EXTRA[@]}"} "$@"
