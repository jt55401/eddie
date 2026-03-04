#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <eleventy-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/src/assets/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/src/assets/eddie/eddie-widget.js"

TARGET_LAYOUT="$SITE_DIR/src/_includes/layouts/base.njk"
if [[ ! -f "$TARGET_LAYOUT" ]]; then
  TARGET_LAYOUT="$(grep -R -l "</head>" "$SITE_DIR/src" | head -n1 || true)"
fi

if [[ -n "$TARGET_LAYOUT" ]] && ! grep -q "eddie-widget.js" "$TARGET_LAYOUT"; then
  perl -0777 -i -pe 's#</head>#  <script defer src="/assets/eddie/eddie-widget.js"></script>\n</head>#s' "$TARGET_LAYOUT"
fi
