#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <hugo-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/static/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/static/eddie/eddie-widget.js"

mkdir -p "$SITE_DIR/layouts/partials"
PARTIAL_FILE="$SITE_DIR/layouts/partials/eddie-script.html"
cat > "$PARTIAL_FILE" <<'HTML'
<script defer src="/eddie/eddie-widget.js"></script>
HTML

BASEOF="$SITE_DIR/layouts/_default/baseof.html"
mkdir -p "$(dirname "$BASEOF")"
if [[ ! -f "$BASEOF" ]]; then
  cat > "$BASEOF" <<'TPL'
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ .Title }}</title>
  {{ partial "eddie-script.html" . }}
</head>
<body>
  {{ block "main" . }}{{ .Content }}{{ end }}
</body>
</html>
TPL
elif ! grep -q "eddie-script.html" "$BASEOF"; then
  perl -0777 -i -pe 's#</head>#  {{ partial "eddie-script.html" . }}\n</head>#s' "$BASEOF"
fi
