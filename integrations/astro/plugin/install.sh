#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <astro-site-dir>}"
ASSET_ROOT="${2:-/repo/dist}"

# widget/assets.list (copied into every asset root by widget/build.sh or
# scripts/sync-integration-assets.sh) is the single source of truth for
# which files ship; read it instead of hardcoding names here.
ASSET_LIST="$ASSET_ROOT/assets.list"
if [[ ! -f "$ASSET_LIST" ]]; then
  echo "Missing asset manifest: $ASSET_LIST (run widget/build.sh first)" >&2
  exit 1
fi
mapfile -t ASSETS < <(grep -v '^#' "$ASSET_LIST" | grep -v '^?' | grep -v '^$')

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
for asset in "${ASSETS[@]}"; do
  cp "$ASSET_ROOT/$asset" "$SITE_DIR/public/eddie/$asset"
done

TARGET_FILE=""
for candidate in \
  "$SITE_DIR/src/layouts/Layout.astro" \
  "$SITE_DIR/src/layouts/BaseLayout.astro" \
  "$SITE_DIR/src/pages/index.astro"; do
  if [[ -f "$candidate" ]]; then
    TARGET_FILE="$candidate"
    break
  fi
done

if [[ -z "$TARGET_FILE" ]]; then
  TARGET_FILE="$(grep -R -l "</head>" "$SITE_DIR/src" | head -n1 || true)"
fi

if [[ -n "$TARGET_FILE" ]] && ! grep -q "eddie-boot.js" "$TARGET_FILE"; then
  perl -0777 -i -pe 's#</head>#  <script defer src="/eddie/eddie-boot.js"></script>\n</head>#s' "$TARGET_FILE"
fi
