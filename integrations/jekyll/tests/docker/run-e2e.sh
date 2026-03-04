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

echo "==> Starting Jekyll server"
bundle exec jekyll serve --host 0.0.0.0 --port 4000 >/tmp/jekyll-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:4000 >/tmp/jekyll-home.html; then
    break
  fi
  sleep 2
done

curl -fsS http://127.0.0.1:4000 >/tmp/jekyll-home.html
grep -q "eddie-widget.js" /tmp/jekyll-home.html

echo "Jekyll E2E passed"
