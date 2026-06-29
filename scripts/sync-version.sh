#!/usr/bin/env bash
# Sync the Rust workspace release version into isolated Apple adapter metadata.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ADAPTER="$ROOT/legacy/apple-swift-adapter"
cd "$ROOT"

if [[ -n "${TAGPR_NEXT_VERSION:-}" ]]; then
    VERSION="${TAGPR_NEXT_VERSION#v}"
else
    VERSION="$(awk '
        $0 == "[workspace.package]" { in_workspace_package = 1; next }
        /^\[/ { in_workspace_package = 0 }
        in_workspace_package && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' Cargo.toml)"
fi

if [[ -z "$VERSION" ]]; then
    echo "sync-version: failed to determine version (TAGPR_NEXT_VERSION unset and Cargo.toml has no workspace.package.version)" >&2
    exit 1
fi

echo "sync-version: $VERSION -> legacy/apple-swift-adapter"

VERSION="$VERSION" perl -i -0pe '
    s|(version:\s*")[^"]+|${1}$ENV{VERSION}|;
' "$ADAPTER/Sources/vo/Vo.swift"

VERSION="$VERSION" perl -i -0pe '
    s|(<key>CFBundleShortVersionString</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
    s|(<key>CFBundleVersion</key>\s*<string>)[^<]+|${1}$ENV{VERSION}|;
' "$ADAPTER/Resources/Info.plist"

git add "$ADAPTER/Sources/vo/Vo.swift" "$ADAPTER/Resources/Info.plist"
