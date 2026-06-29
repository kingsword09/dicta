#!/usr/bin/env bash
# Run the Rust workspace checks for the cross-platform vo CLI.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

cargo fmt --all --check
cargo test --workspace "$@"
