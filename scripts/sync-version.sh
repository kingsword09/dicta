#!/usr/bin/env bash
# Sync the canonical version (from Sources/vo/Vo.swift) into Resources/Info.plist.
# Invoked by tagpr after it bumps the versionFile.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep -E 'version:[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' Sources/vo/Vo.swift \
    | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"

if [[ -z "$VERSION" ]]; then
    echo "sync-version: failed to read version from Sources/vo/Vo.swift" >&2
    exit 1
fi

echo "sync-version: $VERSION -> Resources/Info.plist"

VERSION="$VERSION" perl -i -0pe '
    s|(<key>CFBundleShortVersionString</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
    s|(<key>CFBundleVersion</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
' Resources/Info.plist

git add Resources/Info.plist
