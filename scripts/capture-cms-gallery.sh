#!/usr/bin/env bash
set -euo pipefail

# Capture Eddie search-result screenshots for all CMS integrations.
#
# Defaults:
#   - Uses existing Docker images: eddie-<cms>-e2e-main
#   - Writes final README-sized screenshots into assets/gallery/*.png (640x360)
#
# Options:
#   --build-images     Build/update all CMS E2E Docker images first.
#
# Environment overrides:
#   EDDIE_GALLERY_QUERY      Search query to type in the widget.
#   EDDIE_GALLERY_WAIT_MS    Wait time after typing query (non-first CMS).
#   EDDIE_GALLERY_WAIT_FIRST_MS  Wait time for first CMS (model warmup).
#   EDDIE_GALLERY_PORT       Host port used for the temporary site server.
#   EDDIE_GALLERY_WIDTH      Capture width (default 640).
#   EDDIE_GALLERY_HEIGHT     Capture height (default 360).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TOOLS_DIR="/tmp/eddie-gallery-tools"

BUILD_IMAGES=0
if [[ "${1:-}" == "--build-images" ]]; then
  BUILD_IMAGES=1
  shift
fi
if [[ "$#" -gt 0 ]]; then
  echo "Unexpected arguments: $*" >&2
  echo "Usage: $0 [--build-images]" >&2
  exit 2
fi

QUERY="${EDDIE_GALLERY_QUERY:-browser bug office hours}"
WAIT_MS="${EDDIE_GALLERY_WAIT_MS:-20000}"
WAIT_FIRST_MS="${EDDIE_GALLERY_WAIT_FIRST_MS:-70000}"
HOST_PORT="${EDDIE_GALLERY_PORT:-14100}"
WIDTH="${EDDIE_GALLERY_WIDTH:-640}"
HEIGHT="${EDDIE_GALLERY_HEIGHT:-360}"

CMS_DEFAULT=(hugo astro docusaurus mkdocs eleventy jekyll)
CMS_FILTER_RAW="${EDDIE_GALLERY_CMS:-}"
CMS_LIST=()
declare -A INTERNAL_PORTS=(
  [hugo]=1313
  [astro]=4321
  [docusaurus]=3000
  [mkdocs]=8000
  [eleventy]=8080
  [jekyll]=4000
)

if [[ -n "$CMS_FILTER_RAW" ]]; then
  IFS=',' read -r -a CMS_LIST <<<"$CMS_FILTER_RAW"
else
  CMS_LIST=("${CMS_DEFAULT[@]}")
fi

for cms in "${CMS_LIST[@]}"; do
  if [[ -z "${INTERNAL_PORTS[$cms]:-}" ]]; then
    echo "Unsupported CMS in EDDIE_GALLERY_CMS: $cms" >&2
    echo "Allowed values: hugo,astro,docusaurus,mkdocs,eleventy,jekyll" >&2
    exit 2
  fi
done

mkdir -p "$REPO_ROOT/assets/gallery"

cleanup() {
  for cms in "${CMS_LIST[@]}"; do
    docker rm -f "gallery-$cms" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

ensure_playwright() {
  mkdir -p "$TOOLS_DIR"
  cd "$TOOLS_DIR"
  if [[ ! -f "$TOOLS_DIR/package.json" ]]; then
    npm init -y >/dev/null 2>&1
  fi
  if [[ ! -d "$TOOLS_DIR/node_modules/playwright" ]]; then
    npm install playwright >/dev/null
  fi
}

build_images() {
  for cms in "${CMS_LIST[@]}"; do
    local dockerfile="$REPO_ROOT/integrations/$cms/tests/docker/Dockerfile"
    local image="eddie-$cms-e2e-main"
    echo "==> Building $image"
    docker build -f "$dockerfile" -t "$image" "$REPO_ROOT"
  done
}

wait_ready() {
  local cms="$1"
  local timeout_seconds=720
  local start_ts
  start_ts="$(date +%s)"

  while true; do
    if curl -fsS "http://127.0.0.1:$HOST_PORT" >/dev/null 2>&1; then
      return 0
    fi

    if ! docker ps --format '{{.Names}}' | rg -q "^gallery-$cms$"; then
      echo "Container gallery-$cms exited before readiness." >&2
      docker logs "gallery-$cms" >&2 || true
      return 1
    fi

    local now_ts
    now_ts="$(date +%s)"
    if (( now_ts - start_ts > timeout_seconds )); then
      echo "Timed out waiting for gallery-$cms" >&2
      docker logs "gallery-$cms" | tail -n 200 >&2 || true
      return 1
    fi

    sleep 2
  done
}

capture_one() {
  local cms="$1"
  local wait_for_results="$2"
  local image="eddie-$cms-e2e-main"
  local internal_port="${INTERNAL_PORTS[$cms]}"
  local out="$REPO_ROOT/assets/gallery/${cms}-search-readme.png"
  local open_mode="ctrlk"

  echo "==> Capturing $cms"
  docker rm -f "gallery-$cms" >/dev/null 2>&1 || true

  docker run -d --rm \
    --name "gallery-$cms" \
    -p "$HOST_PORT:$internal_port" \
    -v "$REPO_ROOT:/repo" \
    "$image" \
    bash /repo/scripts/gallery/start-cms-demo.sh "$cms" >/dev/null

  wait_ready "$cms"

  (
    cd "$TOOLS_DIR"
    node "$REPO_ROOT/scripts/gallery/capture-shot.js" \
      "http://127.0.0.1:$HOST_PORT" \
      "$out" \
      "$QUERY" \
      "$wait_for_results" \
      "$WIDTH" \
      "$HEIGHT" \
      "$open_mode"
  )

  docker rm -f "gallery-$cms" >/dev/null 2>&1 || true
  echo "   wrote: $out"
}

if [[ "$BUILD_IMAGES" == "1" ]]; then
  build_images
fi

ensure_playwright

first=1
for cms in "${CMS_LIST[@]}"; do
  if [[ "$first" == "1" ]]; then
    capture_one "$cms" "$WAIT_FIRST_MS"
    first=0
  else
    capture_one "$cms" "$WAIT_MS"
  fi
done

echo
echo "Gallery refresh complete."
echo "Images: $REPO_ROOT/assets/gallery/*-search-readme.png"
