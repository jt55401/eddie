#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the Eddie browser widget.
# Produces dist/ with four files ready to deploy alongside a Hugo site.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "==> Building WASM module..."
wasm-pack build "$PROJECT_ROOT" \
  --target no-modules \
  --out-dir "$SCRIPT_DIR/pkg" \
  --out-name eddie \
  --release

echo "==> Assembling dist/..."
mkdir -p "$PROJECT_ROOT/dist"
cp "$SCRIPT_DIR/pkg/eddie_bg.wasm" "$PROJECT_ROOT/dist/eddie.wasm"
cp "$SCRIPT_DIR/pkg/eddie.js"      "$PROJECT_ROOT/dist/eddie-wasm.js"
cp "$SCRIPT_DIR/src/worker.js"            "$PROJECT_ROOT/dist/eddie-worker.js"
cp "$SCRIPT_DIR/src/eddie-widget.js"      "$PROJECT_ROOT/dist/eddie-widget.js"

echo "==> Build complete. Output:"
ls -lh "$PROJECT_ROOT/dist/"
