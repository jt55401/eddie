#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="/repo"
WORKDIR="/tmp/mkdocs-e2e"
SITE_ROOT="$WORKDIR/site"
INSTALL_SOURCE="${EDDIE_INSTALL_SOURCE:-local}"
PACKAGE_VERSION="${EDDIE_PACKAGE_VERSION:-}"
LOCAL_EDDIE_CLI_BIN="$REPO_ROOT/target/release/eddie"

install_eddie_mkdocs() {
  case "$INSTALL_SOURCE" in
    local)
      bash "$REPO_ROOT/integrations/mkdocs/plugin/install.sh" "$SITE_ROOT"
      ;;
    registry)
      local spec="eddie-mkdocs"
      local cli_spec="jt55401-eddie-cli"
      if [[ -n "$PACKAGE_VERSION" ]]; then
        spec="${spec}==${PACKAGE_VERSION}"
        cli_spec="${cli_spec}==${PACKAGE_VERSION}"
      fi
      pip3 install --break-system-packages --no-cache-dir "$spec"
      pip3 install --break-system-packages --no-cache-dir "$cli_spec"
      eddie-mkdocs-install "$SITE_ROOT" "$REPO_ROOT/dist"
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
    if ! command -v eddie >/dev/null 2>&1; then
      echo "Expected eddie CLI to be installed via Python packages." >&2
      exit 1
    fi
    eddie "$@"
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
install_eddie_mkdocs

echo "==> Indexing MkDocs docs"
run_eddie index \
  --cms mkdocs \
  --content-dir "$SITE_ROOT/docs" \
  --output "$SITE_ROOT/docs/eddie/index.ed"
verify_index_and_search "$SITE_ROOT/docs/eddie/index.ed"

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
