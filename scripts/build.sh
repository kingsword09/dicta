#!/usr/bin/env bash
# Build the Rust vo CLI in release mode.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo ">> cargo build --release -p vo-cli"
cargo build --release -p vo-cli

BIN_PATH="$ROOT/target/release/vo"
echo ">> built: $BIN_PATH"
