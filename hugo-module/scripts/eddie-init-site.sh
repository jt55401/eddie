#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Bootstrap local Eddie files in a Hugo site repo.
#
# Usage:
#   bash eddie-init-site.sh [--force] [SITE_DIR]

set -euo pipefail

FORCE=0
SITE_DIR="."

while [[ $# -gt 0 ]]; do
  case "$1" in
    --force)
      FORCE=1
      shift
      ;;
    -h|--help)
      cat <<'USAGE'
Usage: bash eddie-init-site.sh [--force] [SITE_DIR]

Creates site-local Eddie helper files:
  .eddie/local.env           (local, gitignored)
  .eddie/claims.edits.toml   (local, gitignored)
  scripts/eddie-index.sh     (convenience wrapper)

Also appends ignore lines to .gitignore.
USAGE
      exit 0
      ;;
    *)
      SITE_DIR="$1"
      shift
      ;;
  esac
done

SITE_DIR="$(cd "$SITE_DIR" && pwd)"
EDDIE_DIR="$SITE_DIR/.eddie"
SCRIPT_PATH="$SITE_DIR/scripts/eddie-index.sh"
ENV_PATH="$EDDIE_DIR/local.env"
CLAIMS_EDITS_PATH="$EDDIE_DIR/claims.edits.toml"
GITIGNORE_PATH="$SITE_DIR/.gitignore"

mkdir -p "$EDDIE_DIR" "$SITE_DIR/scripts"

write_if_needed() {
  local path="$1"
  local content="$2"

  if [[ -f "$path" && "$FORCE" -ne 1 ]]; then
    echo "skip: $path (already exists)"
    return
  fi

  printf "%s\n" "$content" > "$path"
  echo "write: $path"
}

read -r -d '' ENV_TEMPLATE <<'EOT' || true
# Eddie local settings (machine-specific). Keep this file out of git.

# Binary to execute (from PATH by default)
EDDIE_BIN=eddie

# Site paths (relative to repo root)
EDDIE_CONTENT_DIR=content
EDDIE_OUTPUT=static/eddie/index.ed
EDDIE_CMS=hugo

# Presets bundle a dense model set (+ sparse, + device where noted). Set one
# and leave the manual settings below empty:
#   fast     = MiniLM-L6
#   balanced = bge-small-en-v1.5 + sparse
#   quality  = bge-small-en-v1.5 + sparse + Qwen3-Embedding-0.6B lane
#   gpu      = quality, with --device cuda
EDDIE_PRESET=

# Manual embedding config (ignored when EDDIE_PRESET is set above).
# EDDIE_DENSE_MODELS is space-separated; each entry becomes its own
# --dense-model lane, so more than one model can run side by side.
EDDIE_DENSE_MODELS=sentence-transformers/multi-qa-MiniLM-L6-cos-v1
EDDIE_SPARSE=0
EDDIE_SPARSE_MODEL=
EDDIE_DEVICE=auto
EDDIE_BATCH_SIZE=32
EDDIE_CHUNK_SIZE=256
EDDIE_OVERLAP=32
EDDIE_CHUNK_STRATEGY=heading
# Per-page summary chunk (title + description + headings); 0 disables it.
EDDIE_SUMMARY_LANE=1
# Fusion weights dense,sparse,bm25 to bake into the index (from `eddie eval --sweep`); empty = built-in default.
EDDIE_WEIGHTS=

# Optional embedded sections (1 = enabled, 0 = disabled)
EDDIE_QA=0
EDDIE_CLAIMS=0

# How QA entries name the site's owner (passed as --qa-subject when set).
# Synthesis otherwise writes "the author"; visitors ask by name.
EDDIE_QA_SUBJECT=

# Set to 1 to index chunk text without the "{title} — {section}" line
# eddie prepends for the dense, sparse and BM25 arms (--no-title-context).
EDDIE_NO_TITLE_CONTEXT=0

# Optional claims edits file (applied only when EDDIE_CLAIMS=1)
EDDIE_CLAIMS_EDITS=.eddie/claims.edits.toml

# Optional local Ollama synthesis (applied only when EDDIE_QA=1)
EDDIE_QA_OLLAMA_MODEL=
EDDIE_QA_OLLAMA_URL=http://127.0.0.1:11434/api/generate
EDDIE_QA_OLLAMA_MAX_CHUNKS=48
EDDIE_QA_OLLAMA_MAX_PAIRS_PER_CHUNK=3
EDDIE_QA_OLLAMA_TEMPERATURE=0.2

# Optional OpenRouter synthesis (if set, takes precedence over Ollama)
EDDIE_QA_OPENROUTER_MODEL=
EDDIE_QA_OPENROUTER_URL=https://openrouter.ai/api/v1/chat/completions
EDDIE_QA_OPENROUTER_API_KEY_ENV=OPENROUTER_API_KEY
EOT

read -r -d '' CLAIMS_TEMPLATE <<'EOT' || true
# Remove an auto-generated claim by matching any combination of fields.
[[redact]]
predicate = "worked_for"
object = "Old Company"

# Add a manual claim.
[[add]]
subject = "Site Subject"
predicate = "worked_for"
object = "Nike"
evidence = "Manual correction"
source_url = "/about/"
confidence = 1.0
tags = ["manual"]
EOT

read -r -d '' WRAPPER_SCRIPT <<'EOT' || true
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${EDDIE_ENV_FILE:-$ROOT_DIR/.eddie/local.env}"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "Missing $ENV_FILE"
  echo "Run the Hugo module init script first."
  exit 1
fi

# shellcheck disable=SC1090
set -a
source "$ENV_FILE"
set +a

BIN="${EDDIE_BIN:-eddie}"
CONTENT_DIR="${EDDIE_CONTENT_DIR:-content}"
OUTPUT_PATH="${EDDIE_OUTPUT:-static/eddie/index.ed}"
CMS="${EDDIE_CMS:-hugo}"
PRESET="${EDDIE_PRESET:-}"
DENSE_MODELS="${EDDIE_DENSE_MODELS:-sentence-transformers/multi-qa-MiniLM-L6-cos-v1}"
SPARSE="${EDDIE_SPARSE:-0}"
SPARSE_MODEL="${EDDIE_SPARSE_MODEL:-}"
DEVICE="${EDDIE_DEVICE:-auto}"
BATCH_SIZE="${EDDIE_BATCH_SIZE:-32}"
CHUNK_SIZE="${EDDIE_CHUNK_SIZE:-256}"
OVERLAP="${EDDIE_OVERLAP:-32}"
CHUNK_STRATEGY="${EDDIE_CHUNK_STRATEGY:-heading}"
SUMMARY_LANE="${EDDIE_SUMMARY_LANE:-1}"
WEIGHTS="${EDDIE_WEIGHTS:-}"

mkdir -p "$(dirname "$ROOT_DIR/$OUTPUT_PATH")"

CMD=(
  "$BIN" index
  --content-dir "$ROOT_DIR/$CONTENT_DIR"
  --output "$ROOT_DIR/$OUTPUT_PATH"
  --cms "$CMS"
  --chunk-size "$CHUNK_SIZE"
  --overlap "$OVERLAP"
  --chunk-strategy "$CHUNK_STRATEGY"
)

if [[ "$SUMMARY_LANE" != "1" ]]; then
  CMD+=(--no-summary-lane)
fi

if [[ -n "$WEIGHTS" ]]; then
  CMD+=(--weights "$WEIGHTS")
fi

if [[ -n "$PRESET" ]]; then
  CMD+=(--preset "$PRESET")
else
  for model in $DENSE_MODELS; do
    CMD+=(--dense-model "$model")
  done
  if [[ -n "$SPARSE_MODEL" ]]; then
    CMD+=(--sparse-model "$SPARSE_MODEL")
  elif [[ "$SPARSE" == "1" ]]; then
    CMD+=(--sparse)
  fi
  CMD+=(--device "$DEVICE")
  CMD+=(--batch-size "$BATCH_SIZE")
fi

if [[ "${EDDIE_NO_TITLE_CONTEXT:-0}" == "1" ]]; then
  CMD+=(--no-title-context)
fi

if [[ "${EDDIE_QA:-0}" == "1" ]]; then
  CMD+=(--qa)
  if [[ -n "${EDDIE_QA_SUBJECT:-}" ]]; then
    CMD+=(--qa-subject "$EDDIE_QA_SUBJECT")
  fi
  if [[ -n "${EDDIE_QA_OPENROUTER_MODEL:-}" ]]; then
    CMD+=(--qa-openrouter-model "$EDDIE_QA_OPENROUTER_MODEL")
    CMD+=(--qa-openrouter-url "${EDDIE_QA_OPENROUTER_URL:-https://openrouter.ai/api/v1/chat/completions}")
    CMD+=(--qa-openrouter-api-key-env "${EDDIE_QA_OPENROUTER_API_KEY_ENV:-OPENROUTER_API_KEY}")
    CMD+=(--qa-ollama-max-chunks "${EDDIE_QA_OLLAMA_MAX_CHUNKS:-48}")
    CMD+=(--qa-ollama-max-pairs-per-chunk "${EDDIE_QA_OLLAMA_MAX_PAIRS_PER_CHUNK:-3}")
    CMD+=(--qa-ollama-temperature "${EDDIE_QA_OLLAMA_TEMPERATURE:-0.2}")
  elif [[ -n "${EDDIE_QA_OLLAMA_MODEL:-}" ]]; then
    CMD+=(--qa-ollama-model "$EDDIE_QA_OLLAMA_MODEL")
    CMD+=(--qa-ollama-url "${EDDIE_QA_OLLAMA_URL:-http://127.0.0.1:11434/api/generate}")
    CMD+=(--qa-ollama-max-chunks "${EDDIE_QA_OLLAMA_MAX_CHUNKS:-48}")
    CMD+=(--qa-ollama-max-pairs-per-chunk "${EDDIE_QA_OLLAMA_MAX_PAIRS_PER_CHUNK:-3}")
    CMD+=(--qa-ollama-temperature "${EDDIE_QA_OLLAMA_TEMPERATURE:-0.2}")
  fi
fi

if [[ "${EDDIE_CLAIMS:-0}" == "1" ]]; then
  CMD+=(--claims)
  CLAIMS_EDITS_REL="${EDDIE_CLAIMS_EDITS:-.eddie/claims.edits.toml}"
  CLAIMS_EDITS_PATH="$ROOT_DIR/$CLAIMS_EDITS_REL"
  if [[ -f "$CLAIMS_EDITS_PATH" ]]; then
    CMD+=(--claims-edits "$CLAIMS_EDITS_PATH")
  fi
fi

echo "Running: ${CMD[*]}"
"${CMD[@]}"
EOT

write_if_needed "$ENV_PATH" "$ENV_TEMPLATE"
write_if_needed "$CLAIMS_EDITS_PATH" "$CLAIMS_TEMPLATE"
write_if_needed "$SCRIPT_PATH" "$WRAPPER_SCRIPT"
chmod +x "$SCRIPT_PATH"

append_line_if_missing() {
  local line="$1"
  if ! grep -Fqx "$line" "$GITIGNORE_PATH" 2>/dev/null; then
    printf "%s\n" "$line" >> "$GITIGNORE_PATH"
    echo "gitignore+: $line"
  fi
}

if [[ ! -f "$GITIGNORE_PATH" ]]; then
  touch "$GITIGNORE_PATH"
fi

if ! grep -Fq "# Eddie local settings" "$GITIGNORE_PATH"; then
  printf "\n# Eddie local settings\n" >> "$GITIGNORE_PATH"
fi
append_line_if_missing ".eddie/local.env"
append_line_if_missing ".eddie/claims.edits.toml"
append_line_if_missing ".eddie/tune-report.json"

echo
echo "Eddie Hugo local bootstrap complete."
echo "Next steps:"
echo "  1) Edit $ENV_PATH"
echo "  2) Run: bash $SCRIPT_PATH"
