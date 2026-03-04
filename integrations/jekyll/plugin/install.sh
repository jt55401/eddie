#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <jekyll-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/assets/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/assets/eddie/eddie-widget.js"

mkdir -p "$SITE_DIR/_includes"
HEAD_CUSTOM="$SITE_DIR/_includes/head-custom.html"
if ! grep -q "eddie-widget.js" "$HEAD_CUSTOM" 2>/dev/null; then
  cat >> "$HEAD_CUSTOM" <<'HTML'
<script defer src="/assets/eddie/eddie-widget.js"></script>
HTML
fi
