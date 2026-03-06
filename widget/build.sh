#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the Eddie browser widget.
# Produces dist/ with four files ready to deploy alongside a Hugo site.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
NPM_SCOPE="${NPM_SCOPE:-jt55401}"

WASM_PACK_SCOPE_ARGS=()
if [[ -n "$NPM_SCOPE" ]]; then
  WASM_PACK_SCOPE_ARGS+=(--scope "$NPM_SCOPE")
fi

echo "==> Building WASM module..."
wasm-pack build "$PROJECT_ROOT" \
  "${WASM_PACK_SCOPE_ARGS[@]}" \
  --target no-modules \
  --out-dir "$SCRIPT_DIR/pkg" \
  --out-name eddie \
  --release

WASM_BIN="$SCRIPT_DIR/pkg/eddie_bg.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  echo "==> Running candidate WASM optimization (wasm-opt -Oz --all-features)..."
  WASM_OPT_CANDIDATE="$(mktemp)"
  cp "$WASM_BIN" "$WASM_OPT_CANDIDATE"
  wasm-opt -Oz --all-features "$WASM_OPT_CANDIDATE" -o "$WASM_OPT_CANDIDATE"

  if command -v brotli >/dev/null 2>&1; then
    base_br="$(brotli -q 11 -c "$WASM_BIN" | wc -c | tr -d ' ')"
    opt_br="$(brotli -q 11 -c "$WASM_OPT_CANDIDATE" | wc -c | tr -d ' ')"
    if (( opt_br < base_br )); then
      mv "$WASM_OPT_CANDIDATE" "$WASM_BIN"
      echo "==> Applied wasm-opt candidate (brotli bytes: $base_br -> $opt_br)."
    else
      rm -f "$WASM_OPT_CANDIDATE"
      echo "==> Skipped wasm-opt candidate (brotli bytes: $base_br -> $opt_br, no gain)."
    fi
  else
    base_raw="$(wc -c <"$WASM_BIN" | tr -d ' ')"
    opt_raw="$(wc -c <"$WASM_OPT_CANDIDATE" | tr -d ' ')"
    if (( opt_raw < base_raw )); then
      mv "$WASM_OPT_CANDIDATE" "$WASM_BIN"
      echo "==> Applied wasm-opt candidate (raw bytes: $base_raw -> $opt_raw)."
    else
      rm -f "$WASM_OPT_CANDIDATE"
      echo "==> Skipped wasm-opt candidate (raw bytes: $base_raw -> $opt_raw, no gain)."
    fi
  fi
else
  echo "==> wasm-opt not found; skipping optional WASM optimization pass."
fi

echo "==> Assembling dist/..."
mkdir -p "$PROJECT_ROOT/dist"
cp "$SCRIPT_DIR/pkg/eddie_bg.wasm" "$PROJECT_ROOT/dist/eddie.wasm"
cp "$SCRIPT_DIR/pkg/eddie.js"      "$PROJECT_ROOT/dist/eddie-wasm.js"
cp "$SCRIPT_DIR/src/worker.js"            "$PROJECT_ROOT/dist/eddie-worker.js"
cp "$SCRIPT_DIR/src/eddie-widget.js"      "$PROJECT_ROOT/dist/eddie-widget.js"

echo "==> Build complete. Output:"
ls -lh "$PROJECT_ROOT/dist/"
