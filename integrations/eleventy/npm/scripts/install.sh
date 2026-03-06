#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <eleventy-site-dir>}"
ASSET_ROOT="${2:-}"
TEMP_ASSET_ROOT=""
ASSETS=(eddie-widget.js eddie-worker.js eddie-wasm.js eddie.wasm)

cleanup() {
  if [[ -n "$TEMP_ASSET_ROOT" && -d "$TEMP_ASSET_ROOT" ]]; then
    rm -rf "$TEMP_ASSET_ROOT"
  fi
}
trap cleanup EXIT

download_public_assets() {
  local version="${EDDIE_RELEASE_VERSION:-}"
  if [[ -z "$version" ]]; then
    echo "No asset root provided and EDDIE_RELEASE_VERSION is unset." >&2
    exit 1
  fi

  TEMP_ASSET_ROOT="$(mktemp -d)"
  ASSET_ROOT="$TEMP_ASSET_ROOT"

  for asset in "${ASSETS[@]}"; do
    curl -fLSs --retry 3 \
      -o "$ASSET_ROOT/$asset" \
      "https://github.com/jt55401/eddie/releases/download/v${version}/${asset}"
  done
}

if [[ -z "$ASSET_ROOT" ]]; then
  download_public_assets
fi

require_asset() {
  local asset_name="$1"
  local asset_path="$ASSET_ROOT/$asset_name"
  if [[ ! -f "$asset_path" ]]; then
    echo "Missing Eddie asset: $asset_path" >&2
    exit 1
  fi
}

for asset in "${ASSETS[@]}"; do
  require_asset "$asset"
done

mkdir -p "$SITE_DIR/public/eddie"
cp "$ASSET_ROOT/eddie-widget.js" "$SITE_DIR/public/eddie/eddie-widget.js"
cp "$ASSET_ROOT/eddie-worker.js" "$SITE_DIR/public/eddie/eddie-worker.js"
cp "$ASSET_ROOT/eddie-wasm.js" "$SITE_DIR/public/eddie/eddie-wasm.js"
cp "$ASSET_ROOT/eddie.wasm" "$SITE_DIR/public/eddie/eddie.wasm"

TARGET_LAYOUT="$SITE_DIR/_includes/layouts/base.njk"
if [[ ! -f "$TARGET_LAYOUT" ]]; then
  TARGET_LAYOUT="$(grep -R -l "</head>" "$SITE_DIR" | head -n1 || true)"
fi

if [[ -n "$TARGET_LAYOUT" ]] && ! grep -q "eddie-widget.js" "$TARGET_LAYOUT"; then
  perl -0777 -i -pe 's#</head>#  <script defer src="/eddie/eddie-widget.js"></script>\n</head>#s' "$TARGET_LAYOUT"
fi
