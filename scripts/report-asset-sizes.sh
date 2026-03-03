#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Report Eddie artifact sizes and enforce optional budgets.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
REPORT_MD="$DIST_DIR/ASSET_SIZES.md"
REPORT_CSV="$DIST_DIR/asset-sizes.csv"

FILES=(
  "eddie-widget.js"
  "eddie-worker.js"
  "eddie-wasm.js"
  "eddie.wasm"
)

has_brotli=0
if command -v brotli >/dev/null 2>&1; then
  has_brotli=1
fi

mkdir -p "$DIST_DIR"

{
  echo "file,raw_bytes,gzip_bytes,brotli_bytes"
  for name in "${FILES[@]}"; do
    path="$DIST_DIR/$name"
    if [[ ! -f "$path" ]]; then
      echo "missing required file: $path" >&2
      exit 1
    fi
    raw_bytes=$(wc -c <"$path" | tr -d ' ')
    gzip_bytes=$(gzip -9 -c "$path" | wc -c | tr -d ' ')
    if [[ "$has_brotli" -eq 1 ]]; then
      brotli_bytes=$(brotli -q 11 -c "$path" | wc -c | tr -d ' ')
    else
      brotli_bytes=0
    fi
    echo "$name,$raw_bytes,$gzip_bytes,$brotli_bytes"
  done
} >"$REPORT_CSV"

{
  echo "# Eddie Asset Sizes"
  echo
  echo "| Artifact | Raw bytes | Gzip bytes | Brotli bytes |"
  echo "|---|---:|---:|---:|"
  tail -n +2 "$REPORT_CSV" | while IFS=, read -r name raw gzip br; do
    if [[ "$has_brotli" -eq 0 ]]; then
      br="n/a"
    fi
    echo "| \`$name\` | $raw | $gzip | $br |"
  done
  if [[ "$has_brotli" -eq 0 ]]; then
    echo
    echo "_Note: brotli CLI not available; Brotli sizes omitted._"
  fi
} >"$REPORT_MD"

cat "$REPORT_MD"

WASM_RAW_BUDGET_BYTES="${WASM_RAW_BUDGET_BYTES:-3400000}"
WASM_GZIP_BUDGET_BYTES="${WASM_GZIP_BUDGET_BYTES:-1100000}"
WASM_BROTLI_BUDGET_BYTES="${WASM_BROTLI_BUDGET_BYTES:-800000}"

wasm_row="$(grep '^eddie.wasm,' "$REPORT_CSV")"
wasm_raw="$(echo "$wasm_row" | cut -d, -f2)"
wasm_gzip="$(echo "$wasm_row" | cut -d, -f3)"
wasm_br="$(echo "$wasm_row" | cut -d, -f4)"

if (( wasm_raw > WASM_RAW_BUDGET_BYTES )); then
  echo "WASM raw size budget exceeded: $wasm_raw > $WASM_RAW_BUDGET_BYTES" >&2
  exit 1
fi
if (( wasm_gzip > WASM_GZIP_BUDGET_BYTES )); then
  echo "WASM gzip size budget exceeded: $wasm_gzip > $WASM_GZIP_BUDGET_BYTES" >&2
  exit 1
fi
if [[ "$has_brotli" -eq 1 ]] && (( wasm_br > WASM_BROTLI_BUDGET_BYTES )); then
  echo "WASM brotli size budget exceeded: $wasm_br > $WASM_BROTLI_BUDGET_BYTES" >&2
  exit 1
fi

echo
echo "Size budgets passed."
