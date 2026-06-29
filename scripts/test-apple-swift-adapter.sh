#!/usr/bin/env bash
# Compatibility wrapper for the renamed Apple Speech adapter test script.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/test-apple-speech-adapter.sh" "$@"
