#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/mkdocs-e2e"
SITE_ROOT="$WORKDIR/site"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

echo "==> Downloading public MkDocs template"
git clone --depth 1 https://github.com/squidfunk/mkdocs-material.git "$SITE_ROOT"

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/mkdocs/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing MkDocs dependencies"
if [[ -f "$SITE_ROOT/requirements.txt" ]]; then
  pip3 install --break-system-packages --no-cache-dir -r "$SITE_ROOT/requirements.txt"
fi
pip3 install --break-system-packages --no-cache-dir mkdocs mkdocs-material mkdocs-minify-plugin

echo "==> Integrating Eddie MkDocs plugin"
bash "$REPO_ROOT/integrations/mkdocs/plugin/install.sh" "$SITE_ROOT"

echo "==> Building Eddie binary"
cd "$REPO_ROOT"
cargo build --release

echo "==> Indexing MkDocs docs"
"$REPO_ROOT/target/release/eddie" index \
  --cms mkdocs \
  --content-dir "$SITE_ROOT/docs" \
  --output "$SITE_ROOT/docs/eddie/index.ed"

echo "==> Building MkDocs site"
cd "$SITE_ROOT"
mkdocs build

echo "==> Starting static site server"
cd "$SITE_ROOT/site"
python3 -m http.server 8000 --bind 0.0.0.0 >/tmp/mkdocs-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:8000 >/tmp/mkdocs-home.html; then
    break
  fi
  sleep 2
done

if ! curl -fsS http://127.0.0.1:8000 >/tmp/mkdocs-home.html; then
  echo "MkDocs server did not come up. Recent logs:" >&2
  tail -n 120 /tmp/mkdocs-server.log >&2 || true
  exit 1
fi
grep -q "eddie-widget.js" /tmp/mkdocs-home.html
curl -fsS http://127.0.0.1:8000/eddie/eddie-worker.js >/tmp/mkdocs-worker.js
curl -fsS http://127.0.0.1:8000/eddie/eddie-wasm.js >/tmp/mkdocs-wasm.js
curl -fsS http://127.0.0.1:8000/eddie/eddie.wasm >/tmp/mkdocs-engine.wasm

echo "MkDocs E2E passed"
