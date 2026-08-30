#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <docusaurus-site-dir>}"
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

mkdir -p "$SITE_DIR/static/eddie"
for asset in "${ASSETS[@]}"; do
  cp "$ASSET_ROOT/$asset" "$SITE_DIR/static/eddie/$asset"
done

mkdir -p "$SITE_DIR/src/theme"
cat > "$SITE_DIR/src/theme/Root.js" <<'ROOT'
import React from 'react';
import Head from '@docusaurus/Head';

export default function Root({children}) {
  return (
    <>
      <Head>
        <script defer src="/eddie/eddie-boot.js" />
      </Head>
      {children}
    </>
  );
}
ROOT
