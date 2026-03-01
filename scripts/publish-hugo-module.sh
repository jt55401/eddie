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
STATIC_DIR="$HUGO_MODULE_DIR/static/static-agent"

TAG=""
TARGET_REPO=""

while [[ $# -gt 0 ]]; do
  case $1 in
    --tag) TAG="$2"; shift 2 ;;
    *)     TARGET_REPO="$1"; shift ;;
  esac
done

# 1. Build WASM
echo "==> Building WASM widget..."
bash "$PROJECT_ROOT/widget/build.sh"

# 2. Copy dist/ into hugo-module/static/static-agent/
echo "==> Assembling Hugo module..."
mkdir -p "$STATIC_DIR"
cp "$PROJECT_ROOT/dist/static-agent.wasm"       "$STATIC_DIR/"
cp "$PROJECT_ROOT/dist/static-agent-wasm.js"    "$STATIC_DIR/"
cp "$PROJECT_ROOT/dist/static-agent-worker.js"  "$STATIC_DIR/"
cp "$PROJECT_ROOT/dist/static-agent-widget.js"  "$STATIC_DIR/"

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

  mkdir -p "$TARGET_REPO/layouts/partials/static-agent"
  cp "$HUGO_MODULE_DIR/layouts/partials/static-agent/inject.html" \
     "$TARGET_REPO/layouts/partials/static-agent/inject.html"

  mkdir -p "$TARGET_REPO/static/static-agent"
  cp "$STATIC_DIR"/* "$TARGET_REPO/static/static-agent/"

  echo "==> Files synced to $TARGET_REPO"

  # 4. Optionally commit and tag
  if [[ -n "$TAG" ]]; then
    cd "$TARGET_REPO"
    git add -A
    if git diff --cached --quiet; then
      echo "==> No changes to commit."
    else
      git commit -m "Release $TAG

Built from static-agent $(git -C "$PROJECT_ROOT" rev-parse --short HEAD)"
      echo "==> Committed release $TAG"
    fi

    git tag -a "$TAG" -m "Release $TAG"
    echo "==> Tagged $TAG — run 'git push && git push --tags' in $TARGET_REPO to publish"
  fi
fi

echo "==> Done."
