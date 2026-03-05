#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <eleventy-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/public/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/public/eddie/eddie-widget.js"

TARGET_LAYOUT="$SITE_DIR/_includes/layouts/base.njk"
if [[ ! -f "$TARGET_LAYOUT" ]]; then
  TARGET_LAYOUT="$(grep -R -l "</head>" "$SITE_DIR" | head -n1 || true)"
fi

if [[ -n "$TARGET_LAYOUT" ]] && ! grep -q "eddie-widget.js" "$TARGET_LAYOUT"; then
  perl -0777 -i -pe 's#</head>#  <script defer src="/eddie/eddie-widget.js"></script>\n</head>#s' "$TARGET_LAYOUT"
fi
