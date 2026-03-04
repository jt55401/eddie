#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the WASM widget and assemble the Hugo module.
#
# Usage:
#   scripts/publish-hugo-module.sh                    # build + assemble locally
#   scripts/publish-hugo-module.sh /path/to/hugo-repo # also sync to separate repo
#   scripts/publish-hugo-module.sh --tag v1.0.0 /path # sync + tag

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HUGO_MODULE_DIR="$PROJECT_ROOT/hugo-module"
STATIC_DIR="$HUGO_MODULE_DIR/static/eddie"

TAG=""
TARGET_REPO=""

while [[ $# -gt 0 ]]; do
  case $1 in
    --tag) TAG="$2"; shift 2 ;;
    *)     TARGET_REPO="$1"; shift ;;
  esac
done

compress_asset() {
  local src="$1"
  if [[ ! -f "$src" ]]; then
    return
  fi

  if command -v brotli >/dev/null 2>&1; then
    brotli -f -q 11 -o "${src}.br" "$src"
  else
    echo "==> brotli not found; skipping ${src}.br"
  fi

  if command -v gzip >/dev/null 2>&1; then
    gzip -n -9 -c "$src" > "${src}.gz"
  else
    echo "==> gzip not found; skipping ${src}.gz"
  fi
}

# 1. Build WASM
echo "==> Building WASM widget..."
bash "$PROJECT_ROOT/widget/build.sh"

echo "==> Generating precompressed assets (.br/.gz)..."
compress_asset "$PROJECT_ROOT/dist/eddie.wasm"
compress_asset "$PROJECT_ROOT/dist/eddie-wasm.js"
compress_asset "$PROJECT_ROOT/dist/eddie-worker.js"
compress_asset "$PROJECT_ROOT/dist/eddie-widget.js"

# 2. Copy dist/ into hugo-module/static/eddie/
echo "==> Assembling Hugo module..."
mkdir -p "$STATIC_DIR"
for asset in \
  eddie.wasm \
  eddie.wasm.br \
  eddie.wasm.gz \
  eddie-wasm.js \
  eddie-wasm.js.br \
  eddie-wasm.js.gz \
  eddie-worker.js \
  eddie-worker.js.br \
  eddie-worker.js.gz \
  eddie-widget.js \
  eddie-widget.js.br \
  eddie-widget.js.gz
do
  if [[ -f "$PROJECT_ROOT/dist/$asset" ]]; then
    cp "$PROJECT_ROOT/dist/$asset" "$STATIC_DIR/"
  fi
done

echo "==> Hugo module assembled at: $HUGO_MODULE_DIR"
ls -lh "$STATIC_DIR/"

# 3. If a target repo path is given, sync files there
if [[ -n "$TARGET_REPO" ]]; then
  if [[ ! -d "$TARGET_REPO" ]]; then
    echo "Error: target repo directory does not exist: $TARGET_REPO"
    exit 1
  fi

  echo "==> Syncing to $TARGET_REPO..."

  # Sync boilerplate (only if not already present or if ours is newer)
  cp "$HUGO_MODULE_DIR/go.mod"    "$TARGET_REPO/go.mod"
  cp "$HUGO_MODULE_DIR/hugo.toml" "$TARGET_REPO/hugo.toml"

  mkdir -p "$TARGET_REPO/layouts/partials/eddie"
  cp "$HUGO_MODULE_DIR/layouts/partials/eddie/inject.html" \
     "$TARGET_REPO/layouts/partials/eddie/inject.html"

  mkdir -p "$TARGET_REPO/static/eddie"
  cp "$STATIC_DIR"/* "$TARGET_REPO/static/eddie/"

  mkdir -p "$TARGET_REPO/scripts"
  cp "$HUGO_MODULE_DIR/scripts/eddie-init-site.sh" \
     "$TARGET_REPO/scripts/eddie-init-site.sh"

  echo "==> Files synced to $TARGET_REPO"

  # 4. Optionally commit and tag
  if [[ -n "$TAG" ]]; then
    cd "$TARGET_REPO"
    git add -A
    if git diff --cached --quiet; then
      echo "==> No changes to commit."
    else
      git commit -m "Release $TAG

Built from eddie $(git -C "$PROJECT_ROOT" rev-parse --short HEAD)"
      echo "==> Committed release $TAG"
    fi

    git tag -a "$TAG" -m "Release $TAG"
    echo "==> Tagged $TAG — run 'git push && git push --tags' in $TARGET_REPO to publish"
  fi
fi

echo "==> Done."
