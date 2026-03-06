#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/astro-e2e"
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

install_eddie_astro() {
  case "$INSTALL_SOURCE" in
    local)
      bash "$REPO_ROOT/integrations/astro/plugin/install.sh" "$SITE_ROOT"
      ;;
    registry)
      npx -y "$(npm_package_spec "@jt55401/eddie-astro")" "$SITE_ROOT"
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

echo "==> Downloading public Astro template"
npm create astro@latest "$SITE_ROOT" -- --template blog --yes --no-install

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/astro/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing Node dependencies"
cd "$SITE_ROOT"
npm install

echo "==> Integrating Eddie Astro plugin"
install_eddie_astro

echo "==> Indexing Astro content"
CONTENT_DIR="$SITE_ROOT/src/content"
if [[ ! -d "$CONTENT_DIR" ]]; then
  CONTENT_DIR="$SITE_ROOT/src/pages"
fi
run_eddie index \
  --cms astro \
  --content-dir "$CONTENT_DIR" \
  --output "$SITE_ROOT/public/eddie/index.ed"
verify_index_and_search "$SITE_ROOT/public/eddie/index.ed"

echo "==> Starting Astro dev server"
cd "$SITE_ROOT"
npm run dev -- --host 0.0.0.0 --port 4321 >/tmp/astro-server.log 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:4321 >/tmp/astro-home.html; then
    break
  fi
  sleep 2
done

if ! curl -fsS http://127.0.0.1:4321 >/tmp/astro-home.html; then
  echo "Astro server did not come up. Recent logs:" >&2
  tail -n 120 /tmp/astro-server.log >&2 || true
  exit 1
fi
grep -q "eddie-widget.js" /tmp/astro-home.html
curl -fsS http://127.0.0.1:4321/eddie/eddie-worker.js >/tmp/astro-worker.js
curl -fsS http://127.0.0.1:4321/eddie/eddie-wasm.js >/tmp/astro-wasm.js
curl -fsS http://127.0.0.1:4321/eddie/eddie.wasm >/tmp/astro-engine.wasm

echo "Astro E2E passed"
