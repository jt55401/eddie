#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the widget runtime once and copy dist/ into every CMS package's
# assets/ directory, per .github/publish-packages.json's "assets_dir" field.
# Run this before packing/publishing any npm/PyPI/RubyGems CMS package.
#
# Usage:
#   scripts/sync-integration-assets.sh            # build widget + copy
#   scripts/sync-integration-assets.sh --no-build  # reuse existing dist/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$PROJECT_ROOT/dist"

DO_BUILD=1
if [[ "${1:-}" == "--no-build" ]]; then
  DO_BUILD=0
fi

if [[ "$DO_BUILD" -eq 1 ]]; then
  echo "==> Building widget runtime (dist/)..."
  bash "$PROJECT_ROOT/widget/build.sh"
fi

ASSETS=(eddie-widget.js eddie-worker.js eddie-wasm.js eddie.wasm)
for asset in "${ASSETS[@]}"; do
  if [[ ! -f "$DIST_DIR/$asset" ]]; then
    echo "Missing built asset: $DIST_DIR/$asset (run without --no-build first)" >&2
    exit 1
  fi
done

ASSET_DIRS="$(python3 -c '
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
dirs = []
for targets in data.values():
    for target in targets:
        d = target.get("assets_dir")
        if d:
            dirs.append(d)
print("\n".join(dirs))
' "$PROJECT_ROOT/.github/publish-packages.json")"

if [[ -z "$ASSET_DIRS" ]]; then
  echo "No assets_dir entries found in .github/publish-packages.json; nothing to sync." >&2
  exit 1
fi

while IFS= read -r rel_dir; do
  [[ -z "$rel_dir" ]] && continue
  target_dir="$PROJECT_ROOT/$rel_dir"
  echo "==> Syncing widget assets into $rel_dir"
  mkdir -p "$target_dir"
  for asset in "${ASSETS[@]}"; do
    cp "$DIST_DIR/$asset" "$target_dir/$asset"
  done
done <<<"$ASSET_DIRS"

echo "==> Done."
