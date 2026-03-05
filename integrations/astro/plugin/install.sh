#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <astro-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/public/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/public/eddie/eddie-widget.js"

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

if [[ -n "$TARGET_FILE" ]] && ! grep -q "eddie-widget.js" "$TARGET_FILE"; then
  perl -0777 -i -pe 's#</head>#  <script defer src="/eddie/eddie-widget.js"></script>\n</head>#s' "$TARGET_FILE"
fi
