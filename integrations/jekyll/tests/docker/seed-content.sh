#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"

python3 /repo/integrations/shared/scripts/render_eddie_corpus.py \
  --cms jekyll \
  --site-root "$SITE_ROOT"
