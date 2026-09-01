#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <jekyll-site-dir>}"
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

mkdir -p "$SITE_DIR/assets/eddie"
for asset in "${ASSETS[@]}"; do
  cp "$ASSET_ROOT/$asset" "$SITE_DIR/assets/eddie/$asset"
done

mkdir -p "$SITE_DIR/_includes"
HEAD_INCLUDE="$SITE_DIR/_includes/head.html"
if [[ -f "$HEAD_INCLUDE" ]]; then
  if ! grep -q "eddie-boot.js" "$HEAD_INCLUDE"; then
    perl -0777 -i -pe 's#</head>#  <script defer src="/assets/eddie/eddie-boot.js" data-index-url="/assets/eddie/index.ed"></script>\n</head>#s' "$HEAD_INCLUDE"
  fi
else
  cat > "$HEAD_INCLUDE" <<'HTML'
<head>
  <meta charset="utf-8">
  <meta http-equiv="X-UA-Compatible" content="IE=edge">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  {%- seo -%}
  <link rel="stylesheet" href="{{ "/assets/main.css" | relative_url }}">
  {%- feed_meta -%}
  {%- if jekyll.environment == 'production' and site.google_analytics -%}
    {%- include google-analytics.html -%}
  {%- endif -%}
  <script defer src="/assets/eddie/eddie-boot.js" data-index-url="/assets/eddie/index.ed"></script>
</head>
HTML
fi
