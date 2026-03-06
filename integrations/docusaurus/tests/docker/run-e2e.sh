#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/docusaurus-e2e"
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

install_eddie_docusaurus() {
  case "$INSTALL_SOURCE" in
    local)
      bash "$REPO_ROOT/integrations/docusaurus/plugin/install.sh" "$SITE_ROOT"
      ;;
    registry)
      npx -y "$(npm_package_spec "@jt55401/eddie-docusaurus")" "$SITE_ROOT" "$REPO_ROOT/dist"
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

echo "==> Downloading public Docusaurus template"
npx create-docusaurus@latest "$SITE_ROOT" classic --javascript --package-manager npm --skip-install

echo "==> Seeding Eddie voice content corpus"
bash "$REPO_ROOT/integrations/docusaurus/tests/docker/seed-content.sh" "$SITE_ROOT"

echo "==> Installing Node dependencies"
cd "$SITE_ROOT"
npm install

echo "==> Integrating Eddie Docusaurus plugin"
install_eddie_docusaurus

echo "==> Indexing Docusaurus docs"
run_eddie index \
  --cms docusaurus \
  --content-dir "$SITE_ROOT/docs" \
  --output "$SITE_ROOT/static/eddie/index.ed"
verify_index_and_search "$SITE_ROOT/static/eddie/index.ed"

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
curl -fsS http://127.0.0.1:3000/eddie/eddie-worker.js >/tmp/docusaurus-worker.js
curl -fsS http://127.0.0.1:3000/eddie/eddie-wasm.js >/tmp/docusaurus-wasm.js
curl -fsS http://127.0.0.1:3000/eddie/eddie.wasm >/tmp/docusaurus-engine.wasm

echo "Docusaurus E2E passed"
