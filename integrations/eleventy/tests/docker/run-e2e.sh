#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/eleventy-e2e"
SITE_ROOT="$WORKDIR/site"
INSTALL_SOURCE="${EDDIE_INSTALL_SOURCE:-local}"
PACKAGE_VERSION="${EDDIE_PACKAGE_VERSION:-}"
LOCAL_EDDIE_CLI_BIN="$REPO_ROOT/target/release/eddie"

npm_package_spec() {
  local package_name="$1"
  if [[ -n "$PACKAGE_VERSION" ]]; then
    printf "%s@%s" "$package_name" "$PACKAGE_VERSION"
  else
    printf "%s" "$package_name"
  fi
}

install_eddie_eleventy() {
  case "$INSTALL_SOURCE" in
    local)
      bash "$REPO_ROOT/integrations/eleventy/plugin/install.sh" "$SITE_ROOT"
      ;;
    registry)
      npx -y "$(npm_package_spec "@jt55401/eddie-eleventy")" "$SITE_ROOT"
      ;;
    *)
      echo "Unsupported EDDIE_INSTALL_SOURCE: $INSTALL_SOURCE" >&2
      exit 2
      ;;
  esac
}

ensure_local_eddie_cli() {
  if [[ -x "$LOCAL_EDDIE_CLI_BIN" ]]; then
    return 0
  fi
  echo "==> Building Eddie binary"
  cd "$REPO_ROOT"
  cargo build --release --locked --bin eddie
}

run_eddie() {
  if [[ "$INSTALL_SOURCE" == "registry" ]]; then
    npx -y "$(npm_package_spec "@jt55401/eddie-cli")" "$@"
  else
    ensure_local_eddie_cli
    "$LOCAL_EDDIE_CLI_BIN" "$@"
  fi
}

verify_index_and_search() {
  local index_path="$1"
  local output
  output="$(run_eddie search \
    --index "$index_path" \
    --query "Revelance" \
    --mode keyword \
    --top-k 10 2>&1)"
  echo "$output" | grep -qi "Queue Before Coffee"
}

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
install_eddie_eleventy

echo "==> Indexing Eleventy content"
run_eddie index \
  --cms eleventy \
  --content-dir "$SITE_ROOT/src" \
  --output "$SITE_ROOT/public/eddie/index.ed"
verify_index_and_search "$SITE_ROOT/public/eddie/index.ed"

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
curl -fsS http://127.0.0.1:8080/eddie/eddie-worker.js >/tmp/eleventy-worker.js
curl -fsS http://127.0.0.1:8080/eddie/eddie-wasm.js >/tmp/eleventy-wasm.js
curl -fsS http://127.0.0.1:8080/eddie/eddie.wasm >/tmp/eleventy-engine.wasm

echo "Eleventy E2E passed"
