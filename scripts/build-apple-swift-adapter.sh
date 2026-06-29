#!/usr/bin/env bash
# Compatibility wrapper for the renamed Apple Speech adapter build script.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/build-apple-speech-adapter.sh" "$@"
