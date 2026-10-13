#!/bin/bash
# Build ORION core workspace

set -e

cd "$(dirname "$0")/../core"

echo "Building ORION core workspace..."
cargo build --release --workspace

echo ""
echo "Build complete! Binaries in target/release/"
ls -lh target/release/orion-core 2>/dev/null || echo "orion-core not found (expected if main.rs not in workspace root)"

echo ""
echo "Workspace members:"
cargo metadata --format-version=1 | jq -r '.packages[] | select(.manifest_path | contains("core/")) | .name'