#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/hugo-e2e"
SITE_ROOT="$WORKDIR/site"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

echo "==> Downloading public Hugo template"
git clone --depth 1 https://github.com/adityatelange/hugo-PaperMod.git "$WORKDIR/theme-src"
hugo new site "$SITE_ROOT"
cp -R "$WORKDIR/theme-src" "$SITE_ROOT/themes/PaperMod"

cat > "$SITE_ROOT/hugo.toml" <<'TOML'
baseURL = "http://example.org/"
languageCode = "en-us"
title = "Eddie Search Logbook"
theme = "PaperMod"
TOML

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/hugo/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Integrating Eddie Hugo plugin"
bash "$REPO_ROOT/integrations/hugo/plugin/install.sh" "$SITE_ROOT"

echo "==> Building Eddie binary"
cd "$REPO_ROOT"
cargo build --release

echo "==> Indexing Hugo content"
"$REPO_ROOT/target/release/eddie" index \
  --cms hugo \
  --content-dir "$SITE_ROOT/content" \
  --output "$SITE_ROOT/static/eddie/index.ed"

echo "==> Building Hugo site"
cd "$SITE_ROOT"
hugo

echo "==> Starting Hugo server"
hugo server --bind 0.0.0.0 --port 1313 >/tmp/hugo-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:1313 >/tmp/hugo-home.html; then
    break
  fi
  sleep 2
done

curl -fsS http://127.0.0.1:1313 >/tmp/hugo-home.html
grep -q "eddie-widget.js" /tmp/hugo-home.html
curl -fsS http://127.0.0.1:1313/eddie/eddie-worker.js >/tmp/hugo-worker.js
curl -fsS http://127.0.0.1:1313/eddie/eddie-wasm.js >/tmp/hugo-wasm.js
curl -fsS http://127.0.0.1:1313/eddie/eddie.wasm >/tmp/hugo-engine.wasm

echo "Hugo E2E passed"
