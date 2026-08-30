#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the Eddie browser widget.
# Produces dist/ with eight files ready to deploy alongside a Hugo site:
#   eddie.wasm, eddie-wasm.js, eddie-worker.js, eddie-widget.js,
#   eddie-agent-worker.js (the page-side hosts) and
#   eddie-sw.js, eddie-wasm-esm.js, eddie-transformers-sw.js (the service
#   worker host; see widget/README.md "Persistent engines").

set -euo pipefail

# `--js-only` skips the WASM build and only re-assembles the JS bundles from
# an existing widget/pkg/ (fast iteration on the widget and workers).
JS_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --js-only) JS_ONLY=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
NPM_SCOPE="${NPM_SCOPE:-jt55401}"

# The service worker cannot import() (HTML spec), so it statically imports a
# copy of transformers.js whose onnxruntime-web imports are redirected to the
# "bundle" build, which carries its WASM binding inline. Pinned by version
# and SHA-256; the version must match TRANSFORMERS_URL in widget/src/worker.js.
TRANSFORMERS_VERSION="4.2.0"
TRANSFORMERS_WEB_SHA256="25e0cbdf5df922996299fcd2cf835101ba979b134389a0dcc54f92022ca7e0ff"
# The exact onnxruntime-web version that transformers.js release depends on
# (its package.json `dependencies`); checked against the manifest when the
# copy is fetched, so bumping TRANSFORMERS_VERSION forces this pin to follow.
ORT_VERSION="1.26.0-dev.20260416-b7804b056c"
VENDOR_DIR="$SCRIPT_DIR/vendor"

WASM_PACK_SCOPE_ARGS=()
if [[ -n "$NPM_SCOPE" ]]; then
  WASM_PACK_SCOPE_ARGS+=(--scope "$NPM_SCOPE")
fi

# Cargo.toml's release profile is opt-level=3 for the native CLI; the WASM
# build trades speed for size. Override either by exporting the variable.
# (panic=abort was measured on 2026-08-28: +0 bytes after brotli, so it stays off.)
export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-s}"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

if (( JS_ONLY )); then
  [[ -f "$SCRIPT_DIR/pkg/eddie_bg.wasm" ]] || { echo "--js-only needs an earlier full build (widget/pkg/ is missing)" >&2; exit 1; }
  [[ -f "$SCRIPT_DIR/pkg-esm/eddie.js" ]] || { echo "--js-only needs an earlier full build (widget/pkg-esm/ is missing)" >&2; exit 1; }
else
echo "==> Building WASM module (opt-level=$CARGO_PROFILE_RELEASE_OPT_LEVEL)..."
wasm-pack build "$PROJECT_ROOT" \
  "${WASM_PACK_SCOPE_ARGS[@]}" \
  --target no-modules \
  --out-dir "$SCRIPT_DIR/pkg" \
  --out-name eddie \
  --release

# Second wasm-bindgen pass for the service worker: ES-module glue over the
# same Rust build. wasm-bindgen emits the same binary for both targets (only
# the JS differs); the hashes are compared before the optimisation pass so
# the service worker can init() against the one eddie.wasm.
echo "==> Building ES-module WASM glue (--target web)..."
wasm-pack build "$PROJECT_ROOT" \
  "${WASM_PACK_SCOPE_ARGS[@]}" \
  --target web \
  --out-dir "$SCRIPT_DIR/pkg-esm" \
  --out-name eddie \
  --release
ESM_WASM_SAME=1
if [[ "$(sha256_of "$SCRIPT_DIR/pkg/eddie_bg.wasm")" != "$(sha256_of "$SCRIPT_DIR/pkg-esm/eddie_bg.wasm")" ]]; then
  ESM_WASM_SAME=0
  echo "==> Note: --target web produced a different binary; dist/ gets a separate eddie-esm.wasm."
fi
echo "$ESM_WASM_SAME" > "$SCRIPT_DIR/pkg-esm/.same-binary"

optimize_wasm() {
  local WASM_BIN="$1"
  if command -v wasm-opt >/dev/null 2>&1; then
    echo "==> Running candidate WASM optimization (wasm-opt -Oz --all-features) on $(basename "$WASM_BIN")..."
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
}
optimize_wasm "$SCRIPT_DIR/pkg/eddie_bg.wasm"
if (( ! ESM_WASM_SAME )); then
  optimize_wasm "$SCRIPT_DIR/pkg-esm/eddie_bg.wasm"
fi
fi # JS_ONLY

# transformers.js copy for the service worker (cached in widget/vendor/).
TF_WEB="$VENDOR_DIR/transformers-$TRANSFORMERS_VERSION.web.js"
if [[ ! -f "$TF_WEB" ]] || [[ "$(sha256_of "$TF_WEB")" != "$TRANSFORMERS_WEB_SHA256" ]]; then
  echo "==> Fetching transformers.js $TRANSFORMERS_VERSION (web build) for the service worker..."
  mkdir -p "$VENDOR_DIR"
  curl -fsSL "https://cdn.jsdelivr.net/npm/@huggingface/transformers@$TRANSFORMERS_VERSION/dist/transformers.web.js" -o "$TF_WEB.tmp"
  got="$(sha256_of "$TF_WEB.tmp")"
  if [[ "$got" != "$TRANSFORMERS_WEB_SHA256" ]]; then
    echo "transformers.web.js SHA-256 mismatch: expected $TRANSFORMERS_WEB_SHA256, got $got" >&2
    rm -f "$TF_WEB.tmp"
    exit 1
  fi
  mv "$TF_WEB.tmp" "$TF_WEB"
  want_ort="$(curl -fsSL "https://cdn.jsdelivr.net/npm/@huggingface/transformers@$TRANSFORMERS_VERSION/package.json" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).dependencies["onnxruntime-web"]))')"
  if [[ "$want_ort" != "$ORT_VERSION" ]]; then
    echo "transformers.js $TRANSFORMERS_VERSION depends on onnxruntime-web $want_ort; update ORT_VERSION in widget/build.sh" >&2
    exit 1
  fi
fi
if ! grep -q "transformers@$TRANSFORMERS_VERSION\"" "$SCRIPT_DIR/src/worker.js"; then
  echo "widget/src/worker.js pins a different transformers.js version than build.sh ($TRANSFORMERS_VERSION)" >&2
  exit 1
fi
echo "==> transformers.js $TRANSFORMERS_VERSION uses onnxruntime-web $ORT_VERSION"

echo "==> Assembling dist/..."
mkdir -p "$PROJECT_ROOT/dist"
cp "$SCRIPT_DIR/pkg/eddie_bg.wasm" "$PROJECT_ROOT/dist/eddie.wasm"
cp "$SCRIPT_DIR/pkg/eddie.js"      "$PROJECT_ROOT/dist/eddie-wasm.js"
ESM_WASM_FILE="eddie.wasm"
if [[ "$(cat "$SCRIPT_DIR/pkg-esm/.same-binary" 2>/dev/null || echo 1)" != "1" ]]; then
  ESM_WASM_FILE="eddie-esm.wasm"
  cp "$SCRIPT_DIR/pkg-esm/eddie_bg.wasm" "$PROJECT_ROOT/dist/eddie-esm.wasm"
else
  rm -f "$PROJECT_ROOT/dist/eddie-esm.wasm"
fi
{
  echo "// SPDX-License-Identifier: GPL-3.0-only"
  echo "// Generated by widget/build.sh (wasm-pack --target web); the service worker imports this."
  cat "$SCRIPT_DIR/pkg-esm/eddie.js"
} > "$PROJECT_ROOT/dist/eddie-wasm-esm.js"

ORT_BUNDLE_URL="https://cdn.jsdelivr.net/npm/onnxruntime-web@$ORT_VERSION/dist/ort.webgpu.bundle.min.mjs"
{
  echo "// Generated by widget/build.sh from @huggingface/transformers@$TRANSFORMERS_VERSION dist/transformers.web.js (Apache-2.0),"
  echo "// with its onnxruntime-web imports pointed at the bundle build so it loads without import() (service workers)."
  sed -e "s#import \* as ONNX_WEB from \"onnxruntime-web/webgpu\";#import * as ONNX_WEB from \"$ORT_BUNDLE_URL\";#" \
      -e "s#import { Tensor } from \"onnxruntime-common\";#import { Tensor } from \"$ORT_BUNDLE_URL\";#" \
      "$TF_WEB"
} > "$PROJECT_ROOT/dist/eddie-transformers-sw.js"
if grep -q 'from "onnxruntime' "$PROJECT_ROOT/dist/eddie-transformers-sw.js"; then
  echo "eddie-transformers-sw.js still has a bare onnxruntime import; the rewrite in build.sh needs updating for this transformers.js version" >&2
  exit 1
fi

# Each JS entry point is the concatenation of the pure modules it uses
# (widget/src/lib/*.js, exposed as `EddieLib`) and its main file. No bundler:
# the lib files attach to a lexical `EddieLib` when one is in scope.
# `defines` is a line of constants prepended to the bundle (may be empty).
bundle() {
  local out="$1"; shift
  local main="$1"; shift
  local wrap="$1"; shift
  local defines="$1"; shift
  {
    echo "// SPDX-License-Identifier: GPL-3.0-only"
    echo "// Generated by widget/build.sh from widget/src/$main and widget/src/lib/*.js; edit those instead."
    if [[ "$wrap" == "iife" ]]; then echo "(function () {"; fi
    echo '"use strict";'
    echo "const EddieLib = {};"
    if [[ -n "$defines" ]]; then echo "$defines"; fi
    for lib in "$@"; do
      cat "$SCRIPT_DIR/src/lib/$lib"
      echo
    done
    cat "$SCRIPT_DIR/src/$main"
    if [[ "$wrap" == "iife" ]]; then echo "})();"; fi
  } > "$out"
  node --check "$out"
}

bundle "$PROJECT_ROOT/dist/eddie-worker.js"       worker.js             plain "" urls.js lanes.js download.js search-engine.js
bundle "$PROJECT_ROOT/dist/eddie-widget.js"       eddie-widget.js       iife  "" config.js urls.js lanes.js agent.js transport.js warm.js
bundle "$PROJECT_ROOT/dist/eddie-agent-worker.js" eddie-agent-worker.js plain "" agent.js agent-engine.js
bundle "$PROJECT_ROOT/dist/eddie-sw.js"           eddie-sw.js           plain "const EDDIE_ESM_WASM = \"$ESM_WASM_FILE\";" urls.js lanes.js download.js agent.js search-engine.js agent-engine.js

echo "==> Build complete. Output:"
ls -lh "$PROJECT_ROOT/dist/"
