#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/eleventy-e2e"
SITE_ROOT="$WORKDIR/site"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

echo "==> Downloading public Eleventy template"
git clone --depth 1 https://github.com/11ty/eleventy-base-blog.git "$SITE_ROOT"

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/eleventy/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing Node dependencies"
cd "$SITE_ROOT"
npm install

echo "==> Integrating Eddie Eleventy plugin"
bash "$REPO_ROOT/integrations/eleventy/plugin/install.sh" "$SITE_ROOT"

echo "==> Building Eddie binary"
cd "$REPO_ROOT"
cargo build --release

echo "==> Indexing Eleventy content"
"$REPO_ROOT/target/release/eddie" index \
  --cms eleventy \
  --content-dir "$SITE_ROOT/src" \
  --output "$SITE_ROOT/public/eddie/index.ed"

echo "==> Building Eleventy site"
cd "$SITE_ROOT"
npm run build

echo "==> Starting static site server"
cd "$SITE_ROOT/_site"
python3 -m http.server 8080 --bind 0.0.0.0 >/tmp/eleventy-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:8080 >/tmp/eleventy-home.html; then
    break
  fi
  sleep 2
done

if ! curl -fsS http://127.0.0.1:8080 >/tmp/eleventy-home.html; then
  echo "Eleventy server did not come up. Recent logs:" >&2
  tail -n 120 /tmp/eleventy-server.log >&2 || true
  exit 1
fi

if ! grep -q "eddie-widget.js" /tmp/eleventy-home.html; then
  echo "Eddie widget tag not found in Eleventy home page." >&2
  echo "Recent home page excerpt:" >&2
  head -n 120 /tmp/eleventy-home.html >&2 || true
  echo "Checking generated assets:" >&2
  find "$SITE_ROOT/_site" -maxdepth 4 -type f | grep -E "eddie|index.ed" >&2 || true
  exit 1
fi

echo "Eleventy E2E passed"
