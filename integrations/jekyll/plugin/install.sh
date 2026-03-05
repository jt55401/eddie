#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <jekyll-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/assets/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/assets/eddie/eddie-widget.js"

mkdir -p "$SITE_DIR/_includes"
HEAD_INCLUDE="$SITE_DIR/_includes/head.html"
if [[ -f "$HEAD_INCLUDE" ]]; then
  if ! grep -q "eddie-widget.js" "$HEAD_INCLUDE"; then
    perl -0777 -i -pe 's#</head>#  <script defer src="/assets/eddie/eddie-widget.js"></script>\n</head>#s' "$HEAD_INCLUDE"
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
  <script defer src="/assets/eddie/eddie-widget.js"></script>
</head>
HTML
fi
