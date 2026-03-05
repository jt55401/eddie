#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/docusaurus-e2e"
SITE_ROOT="$WORKDIR/site"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

echo "==> Downloading public Docusaurus template"
npx create-docusaurus@latest "$SITE_ROOT" classic --javascript --package-manager npm --skip-install

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/docusaurus/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing Node dependencies"
cd "$SITE_ROOT"
npm install

echo "==> Integrating Eddie Docusaurus plugin"
bash "$REPO_ROOT/integrations/docusaurus/plugin/install.sh" "$SITE_ROOT"

echo "==> Building Eddie binary"
cd "$REPO_ROOT"
cargo build --release

echo "==> Indexing Docusaurus docs"
"$REPO_ROOT/target/release/eddie" index \
  --cms docusaurus \
  --content-dir "$SITE_ROOT/docs" \
  --output "$SITE_ROOT/static/eddie/index.ed"

echo "==> Building Docusaurus site"
cd "$SITE_ROOT"
npm run build

echo "==> Starting Docusaurus server"
npm run serve -- --host 0.0.0.0 --port 3000 >/tmp/docusaurus-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:3000 >/tmp/docusaurus-home.html; then
    break
  fi
  sleep 2
done

curl -fsS http://127.0.0.1:3000 >/tmp/docusaurus-home.html
grep -q "eddie-widget.js" /tmp/docusaurus-home.html

echo "Docusaurus E2E passed"
