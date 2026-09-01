#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Report Eddie runtime asset sizes (dist/ASSET_SIZES.md, dist/asset-sizes.csv)
# and enforce the default-path budgets. Each budget is an environment
# variable (bytes); the defaults are the 2026-08-30 measurements plus about
# 15 % headroom (see docs/plans/2026-08-30-efficient-defaults.md).
#
# Default path (what a visitor pays without opting into anything):
#   eddie-boot.js     every page view                       brotli
#   eddie-widget.js   first interaction                     brotli
#   eddie-worker.js   first open (page-worker host)         brotli
#   eddie-lite.wasm   first open                            brotli
# Opt-in (after consent):
#   eddie-dense.wasm  CPU dense lane                        raw, gzip, brotli

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
REPORT_MD="$DIST_DIR/ASSET_SIZES.md"
REPORT_CSV="$DIST_DIR/asset-sizes.csv"

# widget/assets.list is the single source of truth for dist/'s file list;
# every consumer (this script included) reads it instead of hardcoding
# names. Only the required entries are reported here -- the two
# conditionally-produced esm-wasm variants (marked with a leading "?" in
# the list) are not part of the default-path or dense-lane budgets below.
mapfile -t FILES < <(grep -v '^#' "$ROOT_DIR/widget/assets.list" | grep -v '^?' | grep -v '^$')

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

# Budgets (bytes), tightened to the pass-2 measurements plus about 10 %, so a
# regression is caught rather than absorbed. Measured 2026-09-01,
# opt-level=s, no wasm-opt: boot 3,330 br, widget 25,943 br (26,149 with the
# asset-version stamp), worker 15,056 br, lite wasm 200,086 br, dense wasm
# 3,596,914 raw / 1,068,449 gzip / 731,823 br, sw-gpu 16,622 br, sw-agent
# 8,724 br.
BOOT_BROTLI_BUDGET_BYTES="${BOOT_BROTLI_BUDGET_BYTES:-3700}"
WIDGET_BROTLI_BUDGET_BYTES="${WIDGET_BROTLI_BUDGET_BYTES:-28500}"
WORKER_BROTLI_BUDGET_BYTES="${WORKER_BROTLI_BUDGET_BYTES:-16500}"
LITE_WASM_BROTLI_BUDGET_BYTES="${LITE_WASM_BROTLI_BUDGET_BYTES:-215000}"
WASM_RAW_BUDGET_BYTES="${WASM_RAW_BUDGET_BYTES:-3700000}"
WASM_GZIP_BUDGET_BYTES="${WASM_GZIP_BUDGET_BYTES:-1150000}"
WASM_BROTLI_BUDGET_BYTES="${WASM_BROTLI_BUDGET_BYTES:-780000}"
# The gpu tier's budget is really a structural guard: it is what fails if
# WebLLM (or anything else belonging to another tier) is imported into the
# WebGPU *search* worker again. It was 21,187 when it did.
SW_GPU_BROTLI_BUDGET_BYTES="${SW_GPU_BROTLI_BUDGET_BYTES:-18500}"
SW_AGENT_BROTLI_BUDGET_BYTES="${SW_AGENT_BROTLI_BUDGET_BYTES:-10000}"

col() { grep "^$1," "$REPORT_CSV" | cut -d, -f"$2"; }

failed=0
check() {
  local label="$1" actual="$2" budget="$3"
  if (( actual > budget )); then
    echo "budget exceeded: $label $actual > $budget bytes" >&2
    failed=1
  else
    echo "budget ok: $label $actual <= $budget bytes"
  fi
}

echo
check "eddie-dense.wasm raw" "$(col eddie-dense.wasm 2)" "$WASM_RAW_BUDGET_BYTES"
check "eddie-dense.wasm gzip" "$(col eddie-dense.wasm 3)" "$WASM_GZIP_BUDGET_BYTES"
if [[ "$has_brotli" -eq 1 ]]; then
  check "eddie-boot.js brotli" "$(col eddie-boot.js 4)" "$BOOT_BROTLI_BUDGET_BYTES"
  check "eddie-widget.js brotli" "$(col eddie-widget.js 4)" "$WIDGET_BROTLI_BUDGET_BYTES"
  check "eddie-worker.js brotli" "$(col eddie-worker.js 4)" "$WORKER_BROTLI_BUDGET_BYTES"
  check "eddie-lite.wasm brotli" "$(col eddie-lite.wasm 4)" "$LITE_WASM_BROTLI_BUDGET_BYTES"
  check "eddie-dense.wasm brotli" "$(col eddie-dense.wasm 4)" "$WASM_BROTLI_BUDGET_BYTES"
  check "eddie-sw-gpu.js brotli" "$(col eddie-sw-gpu.js 4)" "$SW_GPU_BROTLI_BUDGET_BYTES"
  check "eddie-sw-agent.js brotli" "$(col eddie-sw-agent.js 4)" "$SW_AGENT_BROTLI_BUDGET_BYTES"
else
  echo "brotli CLI not available; the brotli budgets were not checked." >&2
fi

if (( failed )); then
  echo "Size budgets failed." >&2
  exit 1
fi
echo "Size budgets passed."
