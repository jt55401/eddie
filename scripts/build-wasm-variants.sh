#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the two browser WASM variants and report their sizes.
#
#   eddie-lite   --no-default-features: index parsing, BM25, learned sparse
#                (WordPiece query tokenizer built in), RRF search, snippets,
#                QA ranking. No model code; the worker supplies dense query
#                vectors (WebGPU lane) or skips the dense arm.
#   eddie-dense  default features: everything above plus the candle BERT
#                embedder for wasm-candle lanes (init_dense_wasm).
#
# Each variant is built for wasm-pack's `no-modules` target (classic
# workers, importScripts) and `web` target (module workers / ESM import).
# Output: $OUT_DIR/<variant>/<target>/  (default widget/pkg-variants/).
#
# Usage: scripts/build-wasm-variants.sh [--out DIR] [--no-opt] [--variants lite,dense] [--targets no-modules,web]
#
# Honours CARGO_TARGET_DIR (set one per checkout) and
# CARGO_PROFILE_RELEASE_OPT_LEVEL (default `s`, like widget/build.sh).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$PROJECT_ROOT/widget/pkg-variants"
RUN_OPT=1
VARIANTS="lite,dense"
TARGETS="no-modules,web"

while (( $# )); do
  case "$1" in
    --out) OUT_DIR="$2"; shift 2 ;;
    --no-opt) RUN_OPT=0; shift ;;
    --variants) VARIANTS="$2"; shift 2 ;;
    --targets) TARGETS="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-s}"
command -v wasm-pack >/dev/null || { echo "wasm-pack is required (cargo install wasm-pack)" >&2; exit 1; }

HAVE_OPT=0
if (( RUN_OPT )) && command -v wasm-opt >/dev/null 2>&1; then HAVE_OPT=1; fi
HAVE_BR=0; command -v brotli >/dev/null 2>&1 && HAVE_BR=1

bytes() { wc -c <"$1" | tr -d ' '; }
gz_bytes() { gzip -9 -c "$1" | wc -c | tr -d ' '; }
br_bytes() { if (( HAVE_BR )); then brotli -q 11 -c "$1" | wc -c | tr -d ' '; else echo "-"; fi; }
kb() { if [[ "$1" == "-" ]]; then echo "-"; else awk -v b="$1" 'BEGIN { printf "%.0f", b / 1024 }'; fi; }

rows=()
IFS=',' read -r -a variant_list <<<"$VARIANTS"
IFS=',' read -r -a target_list <<<"$TARGETS"
for variant in "${variant_list[@]}"; do
  case "$variant" in
    lite)  feature_args=(--no-default-features) ;;
    dense) feature_args=() ;;
    *) echo "unknown variant: $variant (lite|dense)" >&2; exit 2 ;;
  esac
  for target in "${target_list[@]}"; do
    out="$OUT_DIR/$variant/$target"
    echo "==> eddie-$variant ($target, opt-level=$CARGO_PROFILE_RELEASE_OPT_LEVEL)"
    rm -rf "$out"
    wasm-pack build "$PROJECT_ROOT" --target "$target" --out-dir "$out" --out-name "eddie-$variant" --release \
      -- "${feature_args[@]}"
    wasm="$out/eddie-${variant}_bg.wasm"
    if (( HAVE_OPT )); then
      # Same policy as widget/build.sh: keep the wasm-opt result only when it
      # is smaller after brotli (or raw, without brotli).
      candidate="$(mktemp "${TMPDIR:-/tmp}/eddie-wasm-opt.XXXXXX")"
      wasm-opt -Oz --all-features "$wasm" -o "$candidate"
      if (( HAVE_BR )); then before="$(br_bytes "$wasm")"; after="$(br_bytes "$candidate")"; else before="$(bytes "$wasm")"; after="$(bytes "$candidate")"; fi
      if (( after < before )); then mv "$candidate" "$wasm"; echo "    wasm-opt applied ($before -> $after)"; else rm -f "$candidate"; echo "    wasm-opt skipped ($before -> $after, no gain)"; fi
    fi
    rows+=("eddie-$variant|$target|$(bytes "$wasm")|$(gz_bytes "$wasm")|$(br_bytes "$wasm")|$(bytes "$out/eddie-$variant.js")")
  done
done

echo
printf '%-12s %-11s %10s %10s %10s %10s\n' variant target "raw KB" "gzip KB" "brotli KB" "js KB"
for row in "${rows[@]}"; do
  IFS='|' read -r v t raw gz br js <<<"$row"
  printf '%-12s %-11s %10s %10s %10s %10s\n' "$v" "$t" "$(kb "$raw")" "$(kb "$gz")" "$(kb "$br")" "$(kb "$js")"
done
echo
echo "Output under $OUT_DIR/<variant>/<target>/"
