#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the static-agent browser widget.
# Produces dist/ with four files ready to deploy alongside a Hugo site.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> Building WASM module..."
wasm-pack build "$PROJECT_ROOT" \
  --target no-modules \
  --out-dir "$SCRIPT_DIR/pkg" \
  --out-name static_agent \
  --release

echo "==> Assembling dist/..."
mkdir -p "$PROJECT_ROOT/dist"
cp "$SCRIPT_DIR/pkg/static_agent_bg.wasm" "$PROJECT_ROOT/dist/static-agent.wasm"
cp "$SCRIPT_DIR/pkg/static_agent.js"      "$PROJECT_ROOT/dist/static-agent-wasm.js"
cp "$SCRIPT_DIR/src/worker.js"            "$PROJECT_ROOT/dist/static-agent-worker.js"
cp "$SCRIPT_DIR/src/static-agent-widget.js" "$PROJECT_ROOT/dist/static-agent-widget.js"

echo "==> Build complete. Output:"
ls -lh "$PROJECT_ROOT/dist/"
