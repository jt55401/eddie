#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <mkdocs-site-dir>}"
ASSET_ROOT="${2:-}"
PACKAGE_ROOT="${EDDIE_PACKAGE_ROOT:-}"

if [[ -z "$ASSET_ROOT" && -n "$PACKAGE_ROOT" ]]; then
  ASSET_ROOT="$PACKAGE_ROOT/assets"
fi

if [[ -z "$ASSET_ROOT" ]]; then
  echo "No asset root provided and no packaged assets found." >&2
  echo "Pass an explicit asset-root or set EDDIE_PACKAGE_ROOT." >&2
  exit 1
fi

# widget/assets.list (copied alongside the runtime files by
# scripts/sync-integration-assets.sh or the publish-*.yml workflows) is the
# single source of truth for which files ship; read it instead of
# hardcoding names here.
ASSET_LIST="$ASSET_ROOT/assets.list"
if [[ ! -f "$ASSET_LIST" ]]; then
  echo "Missing asset manifest: $ASSET_LIST (asset root not populated?)" >&2
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

mkdir -p "$SITE_DIR/docs/eddie"
for asset in "${ASSETS[@]}"; do
  cp "$ASSET_ROOT/$asset" "$SITE_DIR/docs/eddie/$asset"
done

MKDOCS_CFG="$SITE_DIR/mkdocs.yml"
if [[ ! -f "$MKDOCS_CFG" ]]; then
  echo "mkdocs.yml not found at $MKDOCS_CFG" >&2
  exit 1
fi

if ! grep -q "eddie/eddie-boot.js" "$MKDOCS_CFG"; then
  cat >> "$MKDOCS_CFG" <<'YAML'

extra_javascript:
  - eddie/eddie-boot.js
YAML
fi
