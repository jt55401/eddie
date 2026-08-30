#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <hugo-site-dir>}"
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

mkdir -p "$SITE_DIR/static/eddie"
for asset in "${ASSETS[@]}"; do
  cp "$ASSET_ROOT/$asset" "$SITE_DIR/static/eddie/$asset"
done

mkdir -p "$SITE_DIR/layouts/partials"
PARTIAL_FILE="$SITE_DIR/layouts/partials/eddie-script.html"
cat > "$PARTIAL_FILE" <<'HTML'
<script defer src="/eddie/eddie-boot.js"></script>
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
