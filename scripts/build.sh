#!/usr/bin/env bash
# Build the Rust dicta CLI in release mode.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

echo ">> cargo build --release -p dicta-cli"
cargo build --release -p dicta-cli

BIN_PATH="$ROOT/target/release/dicta"
echo ">> built: $BIN_PATH"
