#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <mkdocs-site-dir>}"
ASSET_ROOT="${2:-/repo/dist}"

require_asset() {
  local asset_name="$1"
  local asset_path="$ASSET_ROOT/$asset_name"
  if [[ ! -f "$asset_path" ]]; then
    echo "Missing Eddie asset: $asset_path" >&2
    exit 1
  fi
}

for asset in eddie-widget.js eddie-worker.js eddie-wasm.js eddie.wasm; do
  require_asset "$asset"
done

mkdir -p "$SITE_DIR/docs/eddie"
cp "$ASSET_ROOT/eddie-widget.js" "$SITE_DIR/docs/eddie/eddie-widget.js"
cp "$ASSET_ROOT/eddie-worker.js" "$SITE_DIR/docs/eddie/eddie-worker.js"
cp "$ASSET_ROOT/eddie-wasm.js" "$SITE_DIR/docs/eddie/eddie-wasm.js"
cp "$ASSET_ROOT/eddie.wasm" "$SITE_DIR/docs/eddie/eddie.wasm"

MKDOCS_CFG="$SITE_DIR/mkdocs.yml"
if [[ ! -f "$MKDOCS_CFG" ]]; then
  echo "mkdocs.yml not found at $MKDOCS_CFG" >&2
  exit 1
fi

if ! grep -q "eddie/eddie-widget.js" "$MKDOCS_CFG"; then
  cat >> "$MKDOCS_CFG" <<'YAML'

extra_javascript:
  - eddie/eddie-widget.js
YAML
fi
