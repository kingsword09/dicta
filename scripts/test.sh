#!/usr/bin/env bash
# Run the Rust workspace checks for the cross-platform vo CLI.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo fmt --all --check
cargo test --workspace "$@"
