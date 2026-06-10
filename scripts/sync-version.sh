#!/usr/bin/env bash
# Sync the version that's about to be released into Resources/Info.plist.
#
# tagpr exports `TAGPR_NEXT_VERSION` (with a `v` prefix) when it invokes this
# `command`, and it runs the command BEFORE bumping the versionFile — so
# reading Sources/vo/Vo.swift here would give us the previous version, not the
# new one (which is exactly what shipped a stale Info.plist on the v0.1.1
# release). Prefer the env var; fall back to the Swift source only for manual
# invocations outside the tagpr flow.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -n "${TAGPR_NEXT_VERSION:-}" ]]; then
    VERSION="${TAGPR_NEXT_VERSION#v}"
else
    VERSION="$(grep -E 'version:[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' Sources/vo/Vo.swift \
        | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"
fi

if [[ -z "$VERSION" ]]; then
    echo "sync-version: failed to determine version (TAGPR_NEXT_VERSION unset and Sources/vo/Vo.swift has no version literal)" >&2
    exit 1
fi

echo "sync-version: $VERSION -> Resources/Info.plist"

VERSION="$VERSION" perl -i -0pe '
    s|(<key>CFBundleShortVersionString</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
    s|(<key>CFBundleVersion</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
' Resources/Info.plist

git add Resources/Info.plist
