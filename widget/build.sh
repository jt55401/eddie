#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the Eddie browser runtime into dist/.
#
# WASM (two variants of src/wasm.rs, each with a classic-worker glue and an
# ES-module glue):
#   eddie-lite.wasm   --no-default-features: index parsing, BM25, learned
#                     sparse (WordPiece query tokenizer built in), RRF,
#                     snippets, QA ranking, sidecars. Every visitor who
#                     opens the search loads this one.
#   eddie-dense.wasm  default features: lite + the candle BERT embedder for
#                     wasm-candle lanes. Fetched only after a visitor accepts
#                     a CPU dense lane.
#   eddie-lite.js / eddie-dense.js         wasm-bindgen --target no-modules
#                     (importScripts; globals `wasm_bindgen` / `wasm_bindgen_dense`)
#   eddie-lite-esm.js / eddie-dense-esm.js wasm-bindgen --target web (static
#                     imports in the service workers)
#
# Page-side scripts:
#   eddie-boot.js          default loader: trigger button + shortcut, fetches
#                          eddie-widget.js on first interaction
#   eddie-widget.js        the full widget
#   eddie-worker.js        search engine in a classic dedicated worker (fallback host)
#   eddie-agent-worker.js  agent in a module worker (fallback host)
#
# Service workers, one source (widget/src/eddie-sw.js) built three times
# with different static imports (import() is not allowed in a service
# worker), each registered in its own scope by lib/transport.js:
#   eddie-sw-lite.js        lite wasm (keyword + sparse search)
#   eddie-sw-dense.js       lite + dense wasm (CPU dense lane)
#   eddie-sw-gpu.js         lite wasm + transformers.js (WebGPU lane) + WebLLM (agent)
#   eddie-transformers-sw.js transformers.js copy the gpu tier imports
#
# Usage: widget/build.sh [--js-only] [--sizes]
#   --js-only  reuse widget/pkg*/ from an earlier build (fast JS iteration)
#   --sizes    print raw / gzip / brotli sizes of every dist file at the end

set -euo pipefail

JS_ONLY=0
SIZES=0
for arg in "$@"; do
  case "$arg" in
    --js-only) JS_ONLY=1 ;;
    --sizes) SIZES=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST="$PROJECT_ROOT/dist"
NPM_SCOPE="${NPM_SCOPE:-jt55401}"

# The gpu service worker statically imports a copy of transformers.js whose
# onnxruntime-web imports are redirected to the "bundle" build, which
# carries its WASM binding inline. Pinned by version and SHA-256; the
# version must match TRANSFORMERS_URL in widget/src/worker.js.
TRANSFORMERS_VERSION="4.2.0"
TRANSFORMERS_WEB_SHA256="25e0cbdf5df922996299fcd2cf835101ba979b134389a0dcc54f92022ca7e0ff"
# The exact onnxruntime-web version that transformers.js release depends on
# (its package.json `dependencies`); checked when the copy is fetched, so
# bumping TRANSFORMERS_VERSION forces this pin to follow.
ORT_VERSION="1.26.0-dev.20260416-b7804b056c"
WEBLLM_URL="https://cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm"
VENDOR_DIR="$SCRIPT_DIR/vendor"

WASM_PACK_SCOPE_ARGS=()
if [[ -n "$NPM_SCOPE" ]]; then
  WASM_PACK_SCOPE_ARGS+=(--scope "$NPM_SCOPE")
fi

# Cargo.toml's release profile is opt-level=3 for the native CLI; the WASM
# build trades speed for size. Override by exporting the variable.
# (panic=abort was measured on 2026-08-28: +0 bytes after brotli, so it stays off.)
export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-s}"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
bytes() { wc -c <"$1" | tr -d ' '; }
br_bytes() { brotli -q 11 -c "$1" | wc -c | tr -d ' '; }
gz_bytes() { gzip -9 -c "$1" | wc -c | tr -d ' '; }

# variant -> wasm-pack out-dir for each target. widget/pkg stays the dense
# no-modules build named `eddie` (the @scope/eddie npm package).
pkg_dir() {
  case "$1/$2" in
    dense/no-modules) echo "$SCRIPT_DIR/pkg" ;;
    dense/web)        echo "$SCRIPT_DIR/pkg-esm" ;;
    lite/no-modules)  echo "$SCRIPT_DIR/pkg-lite" ;;
    lite/web)         echo "$SCRIPT_DIR/pkg-lite-esm" ;;
  esac
}
out_name() { case "$1" in dense) echo eddie ;; lite) echo eddie-lite ;; esac; }

# Try wasm-opt -Oz and keep the result only when it is smaller after brotli
# (raw, without brotli). Measured 2026-08-30: -Oz grows both variants after
# brotli, so this usually reports "skipped"; it stays in case that changes.
optimize_wasm() {
  local WASM_BIN="$1"
  if ! command -v wasm-opt >/dev/null 2>&1; then
    echo "==> wasm-opt not found; skipping optional WASM optimization pass."
    return
  fi
  echo "==> Running candidate WASM optimization (wasm-opt -Oz --all-features) on $(basename "$WASM_BIN")..."
  local cand
  cand="$(mktemp "${TMPDIR:-/tmp}/eddie-wasm-opt.XXXXXX")"
  cp "$WASM_BIN" "$cand"
  wasm-opt -Oz --all-features "$cand" -o "$cand"
  local before after unit
  if command -v brotli >/dev/null 2>&1; then before="$(br_bytes "$WASM_BIN")"; after="$(br_bytes "$cand")"; unit=brotli; else before="$(bytes "$WASM_BIN")"; after="$(bytes "$cand")"; unit=raw; fi
  if (( after < before )); then
    mv "$cand" "$WASM_BIN"
    echo "==> Applied wasm-opt candidate ($unit bytes: $before -> $after)."
  else
    rm -f "$cand"
    echo "==> Skipped wasm-opt candidate ($unit bytes: $before -> $after, no gain)."
  fi
}

if (( JS_ONLY )); then
  for v in lite dense; do
    for t in no-modules web; do
      d="$(pkg_dir "$v" "$t")"
      [[ -f "$d/$(out_name "$v")_bg.wasm" ]] || { echo "--js-only needs an earlier full build ($d is missing)" >&2; exit 1; }
    done
  done
else
  command -v wasm-pack >/dev/null || { echo "wasm-pack is required (cargo install wasm-pack)" >&2; exit 1; }
  for variant in lite dense; do
    features=()
    [[ "$variant" == "lite" ]] && features=(--no-default-features)
    for target in no-modules web; do
      dir="$(pkg_dir "$variant" "$target")"
      echo "==> Building eddie-$variant ($target, opt-level=$CARGO_PROFILE_RELEASE_OPT_LEVEL)..."
      wasm-pack build "$PROJECT_ROOT" "${WASM_PACK_SCOPE_ARGS[@]}" --target "$target" --out-dir "$dir" --out-name "$(out_name "$variant")" --release -- "${features[@]}"
    done
    # wasm-bindgen emits the same binary for both targets (only the JS
    # differs); compare before optimising so one file serves both glues.
    nm="$(pkg_dir "$variant" no-modules)/$(out_name "$variant")_bg.wasm"
    web="$(pkg_dir "$variant" web)/$(out_name "$variant")_bg.wasm"
    same=1
    if [[ "$(sha256_of "$nm")" != "$(sha256_of "$web")" ]]; then
      same=0
      echo "==> Note: --target web produced a different eddie-$variant binary; dist/ gets a separate eddie-$variant-esm.wasm."
    fi
    echo "$same" > "$(pkg_dir "$variant" web)/.same-binary"
    optimize_wasm "$nm"
    (( same )) || optimize_wasm "$web"
  done
fi

# transformers.js copy for the gpu service worker (cached in widget/vendor/).
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
if ! grep -q "web-llm@0.2.84" "$SCRIPT_DIR/src/eddie-agent-worker.js"; then
  echo "widget/src/eddie-agent-worker.js pins a different WebLLM version than build.sh ($WEBLLM_URL)" >&2
  exit 1
fi
echo "==> transformers.js $TRANSFORMERS_VERSION uses onnxruntime-web $ORT_VERSION"

echo "==> Assembling dist/..."
mkdir -p "$DIST"
rm -f "$DIST"/eddie.wasm "$DIST"/eddie-esm.wasm "$DIST"/eddie-wasm.js "$DIST"/eddie-wasm-esm.js "$DIST"/eddie-sw.js \
      "$DIST"/eddie-lite-esm.wasm "$DIST"/eddie-dense-esm.wasm
for variant in lite dense; do
  name="$(out_name "$variant")"
  cp "$(pkg_dir "$variant" no-modules)/${name}_bg.wasm" "$DIST/eddie-$variant.wasm"
  if [[ "$(cat "$(pkg_dir "$variant" web)/.same-binary" 2>/dev/null || echo 1)" != "1" ]]; then
    cp "$(pkg_dir "$variant" web)/${name}_bg.wasm" "$DIST/eddie-$variant-esm.wasm"
  fi
  {
    echo "// SPDX-License-Identifier: GPL-3.0-only"
    echo "// Generated by widget/build.sh (wasm-pack --target no-modules, eddie-$variant); loaded with importScripts by eddie-worker.js."
    if [[ "$variant" == "dense" ]]; then
      # The lite glue already declared `wasm_bindgen` in the worker's global
      # scope; a second `let` of the same name is a SyntaxError.
      sed -e '1s/^let wasm_bindgen = /let wasm_bindgen_dense = /' "$(pkg_dir "$variant" no-modules)/$name.js"
    else
      cat "$(pkg_dir "$variant" no-modules)/$name.js"
    fi
  } > "$DIST/eddie-$variant.js"
  if [[ "$variant" == "dense" ]] && ! grep -q '^let wasm_bindgen_dense = ' "$DIST/eddie-dense.js"; then
    echo "eddie-dense.js: the wasm-bindgen global was not renamed; the glue layout changed, update build.sh" >&2
    exit 1
  fi
  {
    echo "// SPDX-License-Identifier: GPL-3.0-only"
    echo "// Generated by widget/build.sh (wasm-pack --target web, eddie-$variant); the service workers import this."
    cat "$(pkg_dir "$variant" web)/$name.js"
  } > "$DIST/eddie-$variant-esm.js"
done
LITE_WASM_FILE="eddie-lite.wasm"; [[ -f "$DIST/eddie-lite-esm.wasm" ]] && LITE_WASM_FILE="eddie-lite-esm.wasm"
DENSE_WASM_FILE="eddie-dense.wasm"; [[ -f "$DIST/eddie-dense-esm.wasm" ]] && DENSE_WASM_FILE="eddie-dense-esm.wasm"

ORT_BUNDLE_URL="https://cdn.jsdelivr.net/npm/onnxruntime-web@$ORT_VERSION/dist/ort.webgpu.bundle.min.mjs"
{
  echo "// Generated by widget/build.sh from @huggingface/transformers@$TRANSFORMERS_VERSION dist/transformers.web.js (Apache-2.0),"
  echo "// with its onnxruntime-web imports pointed at the bundle build so it loads without import() (service workers)."
  sed -e "s#import \* as ONNX_WEB from \"onnxruntime-web/webgpu\";#import * as ONNX_WEB from \"$ORT_BUNDLE_URL\";#" \
      -e "s#import { Tensor } from \"onnxruntime-common\";#import { Tensor } from \"$ORT_BUNDLE_URL\";#" \
      "$TF_WEB"
} > "$DIST/eddie-transformers-sw.js"
if grep -q 'from "onnxruntime' "$DIST/eddie-transformers-sw.js"; then
  echo "eddie-transformers-sw.js still has a bare onnxruntime import; the rewrite in build.sh needs updating for this transformers.js version" >&2
  exit 1
fi

# Each JS entry point is the concatenation of the pure modules it uses
# (widget/src/lib/*.js, exposed as `EddieLib`) and its main file. No bundler:
# the lib files attach to a lexical `EddieLib` when one is in scope.
# `defines` is prepended to the bundle (constants, or the static imports of
# a service worker tier; may be empty).
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
    if [[ -n "$defines" ]]; then echo "$defines"; fi
    echo "const EddieLib = {};"
    for lib in "$@"; do
      cat "$SCRIPT_DIR/src/lib/$lib"
      echo
    done
    cat "$SCRIPT_DIR/src/$main"
    if [[ "$wrap" == "iife" ]]; then echo "})();"; fi
  } > "$out"
  if [[ "$wrap" == "module" ]]; then
    # A .mjs copy makes every Node version parse the static imports as ESM.
    local check
    check="$(mktemp "${TMPDIR:-/tmp}/eddie-check.XXXXXX.mjs")"
    cp "$out" "$check"
    node --check "$check"
    rm -f "$check"
  else
    node --check "$out"
  fi
}

bundle "$DIST/eddie-boot.js"         eddie-boot.js         iife  "" boot.js
bundle "$DIST/eddie-widget.js"       eddie-widget.js       iife  "" config.js urls.js lanes.js agent.js transport.js warm.js
bundle "$DIST/eddie-worker.js"       worker.js             plain "" urls.js lanes.js download.js search-engine.js
bundle "$DIST/eddie-agent-worker.js" eddie-agent-worker.js plain "" agent.js agent-engine.js

# The agent (agent.js, agent-engine.js) is bundled into the gpu tier only.
SW_LIBS=(urls.js lanes.js download.js search-engine.js)
SW_GPU_LIBS=("${SW_LIBS[@]}" agent.js agent-engine.js)
SW_LITE_IMPORTS="import initLiteWasm, * as liteWasmApi from \"./eddie-lite-esm.js\";
const EDDIE_LITE_WASM = \"$LITE_WASM_FILE\";"
bundle "$DIST/eddie-sw-lite.js" eddie-sw.js module "$SW_LITE_IMPORTS
const EDDIE_SW_TIER = \"lite\";
const initDenseWasm = null, denseWasmApi = null, EDDIE_DENSE_WASM = null, webllm = null, transformers = null;" "${SW_LIBS[@]}"
bundle "$DIST/eddie-sw-dense.js" eddie-sw.js module "$SW_LITE_IMPORTS
import initDenseWasm, * as denseWasmApi from \"./eddie-dense-esm.js\";
const EDDIE_SW_TIER = \"dense\";
const EDDIE_DENSE_WASM = \"$DENSE_WASM_FILE\", webllm = null, transformers = null;" "${SW_LIBS[@]}"
bundle "$DIST/eddie-sw-gpu.js" eddie-sw.js module "$SW_LITE_IMPORTS
import * as webllm from \"$WEBLLM_URL\";
import * as transformers from \"./eddie-transformers-sw.js\";
const EDDIE_SW_TIER = \"gpu\";
const initDenseWasm = null, denseWasmApi = null, EDDIE_DENSE_WASM = null;" "${SW_GPU_LIBS[@]}"

# widget/assets.list is the single source of truth for dist/'s file list;
# every other piece of release/integration plumbing reads it instead of
# hardcoding names (see the comment at the top of that file). Copy it into
# dist/ so a built dist/ is self-describing even where widget/ itself isn't
# checked out (published CMS packages), then verify the build produced
# exactly what the list promises -- neither a missing required file nor an
# unlisted extra one.
ASSET_LIST="$SCRIPT_DIR/assets.list"
cp "$ASSET_LIST" "$DIST/assets.list"
mapfile -t REQUIRED_ASSETS < <(grep -v '^#' "$ASSET_LIST" | grep -v '^?' | grep -v '^$')
mapfile -t OPTIONAL_ASSETS < <(grep '^?' "$ASSET_LIST" | sed 's/^?//')
manifest_failed=0
for f in "${REQUIRED_ASSETS[@]}"; do
  [[ -f "$DIST/$f" ]] || { echo "widget/assets.list requires $f but build.sh did not produce dist/$f" >&2; manifest_failed=1; }
done
for f in "$DIST"/*; do
  name="$(basename "$f")"
  # assets.list is this step's own output; ASSET_SIZES.md/asset-sizes.csv
  # are scripts/report-asset-sizes.sh's output into the same directory, not
  # part of the widget build -- both are expected bystanders, not drift.
  case "$name" in
    assets.list | ASSET_SIZES.md | asset-sizes.csv) continue ;;
  esac
  listed=0
  for known in "${REQUIRED_ASSETS[@]}" "${OPTIONAL_ASSETS[@]}"; do
    [[ "$name" == "$known" ]] && { listed=1; break; }
  done
  (( listed )) || { echo "dist/$name was produced but is not in widget/assets.list (update the list or fix the build)" >&2; manifest_failed=1; }
done
if (( manifest_failed )); then
  echo "dist/ does not match widget/assets.list; see messages above." >&2
  exit 1
fi

echo "==> Build complete. Output:"
ls -lh "$DIST/"

if (( SIZES )); then
  echo
  printf '%-26s %10s %10s %10s\n' file "raw" "gzip" "brotli"
  for f in "$DIST"/*; do
    if command -v brotli >/dev/null 2>&1; then br="$(br_bytes "$f")"; else br="-"; fi
    printf '%-26s %10s %10s %10s\n' "$(basename "$f")" "$(bytes "$f")" "$(gz_bytes "$f")" "$br"
  done
fi
