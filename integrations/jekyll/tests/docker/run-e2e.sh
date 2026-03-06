#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/jekyll-e2e"
SITE_ROOT="$WORKDIR/site"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

echo "==> Downloading public Jekyll starter template"
jekyll new "$SITE_ROOT" --force

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/jekyll/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing Ruby dependencies"
cd "$SITE_ROOT"
bundle install

echo "==> Integrating Eddie Jekyll plugin"
bash "$REPO_ROOT/integrations/jekyll/plugin/install.sh" "$SITE_ROOT"

echo "==> Building Eddie binary"
cd "$REPO_ROOT"
cargo build --release

echo "==> Indexing Jekyll content"
"$REPO_ROOT/target/release/eddie" index \
  --cms jekyll \
  --content-dir "$SITE_ROOT" \
  --output "$SITE_ROOT/assets/eddie/index.ed"

echo "==> Building Jekyll site"
cd "$SITE_ROOT"
bundle exec jekyll build

echo "==> Starting static site server"
cd "$SITE_ROOT/_site"
python3 -m http.server 4000 --bind 0.0.0.0 >/tmp/jekyll-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

sleep 2
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "Jekyll static server did not stay running. Recent logs:" >&2
  tail -n 120 /tmp/jekyll-server.log >&2 || true
  exit 1
fi

grep -q "eddie-widget.js" "$SITE_ROOT/_site/index.html"
grep -q 'data-index-url="/assets/eddie/index.ed"' "$SITE_ROOT/_site/index.html"
curl -fsS http://127.0.0.1:4000/assets/eddie/index.ed >/tmp/jekyll-index.ed
curl -fsS http://127.0.0.1:4000/assets/eddie/eddie-worker.js >/tmp/jekyll-worker.js
curl -fsS http://127.0.0.1:4000/assets/eddie/eddie-wasm.js >/tmp/jekyll-wasm.js
curl -fsS http://127.0.0.1:4000/assets/eddie/eddie.wasm >/tmp/jekyll-engine.wasm

echo "Jekyll E2E passed"
