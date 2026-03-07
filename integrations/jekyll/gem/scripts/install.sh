#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <jekyll-site-dir>}"
ASSET_ROOT="${2:-}"
PACKAGE_ROOT="${EDDIE_PACKAGE_ROOT:-}"
ASSETS=(eddie-widget.js eddie-worker.js eddie-wasm.js eddie.wasm)

if [[ -z "$ASSET_ROOT" && -n "$PACKAGE_ROOT" ]]; then
  ASSET_ROOT="$PACKAGE_ROOT/assets"
fi

if [[ -z "$ASSET_ROOT" ]]; then
  echo "No asset root provided and no packaged assets found." >&2
  echo "Pass an explicit asset-root or set EDDIE_PACKAGE_ROOT." >&2
  exit 1
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

mkdir -p "$SITE_DIR/assets/eddie"
cp "$ASSET_ROOT/eddie-widget.js" "$SITE_DIR/assets/eddie/eddie-widget.js"
cp "$ASSET_ROOT/eddie-worker.js" "$SITE_DIR/assets/eddie/eddie-worker.js"
cp "$ASSET_ROOT/eddie-wasm.js" "$SITE_DIR/assets/eddie/eddie-wasm.js"
cp "$ASSET_ROOT/eddie.wasm" "$SITE_DIR/assets/eddie/eddie.wasm"

mkdir -p "$SITE_DIR/_includes"
HEAD_INCLUDE="$SITE_DIR/_includes/head.html"
if [[ -f "$HEAD_INCLUDE" ]]; then
  if ! grep -q "eddie-widget.js" "$HEAD_INCLUDE"; then
    perl -0777 -i -pe 's#</head>#  <script defer src="/assets/eddie/eddie-widget.js" data-index-url="/assets/eddie/index.ed"></script>\n</head>#s' "$HEAD_INCLUDE"
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
  <script defer src="/assets/eddie/eddie-widget.js" data-index-url="/assets/eddie/index.ed"></script>
</head>
HTML
fi
