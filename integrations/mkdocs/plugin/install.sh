#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <mkdocs-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/docs/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/docs/eddie/eddie-widget.js"

MKDOCS_CFG="$SITE_DIR/mkdocs.yml"
if [[ ! -f "$MKDOCS_CFG" ]]; then
  echo "mkdocs.yml not found at $MKDOCS_CFG" >&2
  exit 1
fi

if ! grep -q "eddie/eddie-widget.js" "$MKDOCS_CFG"; then
  cat >> "$MKDOCS_CFG" <<'YAML'

extra_javascript:
  - eddie/eddie-widget.js
YAML
fi
