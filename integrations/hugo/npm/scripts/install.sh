#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <hugo-site-dir>}"
ASSET_ROOT="${2:-}"
TEMP_ASSET_ROOT=""
ASSETS=(eddie-widget.js eddie-worker.js eddie-wasm.js eddie.wasm)

cleanup() {
  if [[ -n "$TEMP_ASSET_ROOT" && -d "$TEMP_ASSET_ROOT" ]]; then
    rm -rf "$TEMP_ASSET_ROOT"
  fi
}
trap cleanup EXIT

download_public_assets() {
  local version="${EDDIE_RELEASE_VERSION:-}"
  if [[ -z "$version" ]]; then
    echo "No asset root provided and EDDIE_RELEASE_VERSION is unset." >&2
    exit 1
  fi

  TEMP_ASSET_ROOT="$(mktemp -d)"
  ASSET_ROOT="$TEMP_ASSET_ROOT"

  for asset in "${ASSETS[@]}"; do
    curl -fLSs --retry 3 \
      -o "$ASSET_ROOT/$asset" \
      "https://github.com/jt55401/eddie/releases/download/v${version}/${asset}"
  done
}

if [[ -z "$ASSET_ROOT" ]]; then
  download_public_assets
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

mkdir -p "$SITE_DIR/static/eddie"
cp "$ASSET_ROOT/eddie-widget.js" "$SITE_DIR/static/eddie/eddie-widget.js"
cp "$ASSET_ROOT/eddie-worker.js" "$SITE_DIR/static/eddie/eddie-worker.js"
cp "$ASSET_ROOT/eddie-wasm.js" "$SITE_DIR/static/eddie/eddie-wasm.js"
cp "$ASSET_ROOT/eddie.wasm" "$SITE_DIR/static/eddie/eddie.wasm"

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
