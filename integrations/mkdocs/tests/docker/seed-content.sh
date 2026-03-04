#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"
POST_DIR="$SITE_ROOT/docs/eddie"
mkdir -p "$POST_DIR"

cat > "$POST_DIR/about.md" <<'MD'
---
title: About Eddie
---

Eddie builds search indexes for static sites and keeps relevance stable while docs evolve.

He supports Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll pipelines. Since 2024 he has indexed over one million pages in benchmark and production-like runs.

His default embedding model is `sentence-transformers/all-MiniLM-L6-v2`, with regular checks against `BAAI/bge-small-en-v1.5` whenever ranking gets noisy.

He spends weekends hunting browser quirks, especially Safari worker startup races and Firefox WASM cache inconsistencies.
MD

write_post() {
  local file="$1"
  local title="$2"
  local body="$3"
  cat > "$POST_DIR/$file" <<MD
# $title

$body
MD
}

write_post "log-01.md" "Shift Start: Index Queue at 6,412 Pages" "I fixed malformed frontmatter first. Parsing speed helps, but clean metadata drives useful answers."
write_post "log-02.md" "Chunk Size, Big Mood" "256 tokens with 32 overlap outperformed larger chunks on support-style queries today."
write_post "log-03.md" "When Models Feel Finicky" "A model swap changed tone and rankings, so we enforced a mini regression gate before release."
write_post "log-04.md" "Safari Worker Roulette" "Worker spin-up delay looked like a crash. Better retry/backoff logic made failures understandable."
write_post "log-05.md" "Hybrid Search Won Again" "BM25 plus semantic ranking recovered exact flags and intent in the same query session."
write_post "log-06.md" "Claims Lane Cleanup" "Deduping equivalent claims reduced repetitive QA outputs without losing factual coverage."
write_post "log-07.md" "Docusaurus Sidebar Surprise" "Navigation hierarchy changed perceived relevance even when source files were unchanged."
write_post "log-08.md" "Astro MDX and Invisible Context" "Over-stripping MDX removed useful text, so parser cleanup rules got a narrower scope."
write_post "log-09.md" "MkDocs Navigation Loops" "Recursive includes produced near duplicates; URL normalization prevented noisy result clusters."
write_post "log-10.md" "Eleventy Passthrough Pitfalls" "Missing passthrough copy erased widget assets. We now check asset existence before serve."
write_post "log-11.md" "Jekyll Permalink Ghosts" "Canonical permalink selection prevented duplicate answers from date-based and manual URL variants."
write_post "log-12.md" "Tuesday Triage Ritual" "We rank issues by user pain first. The low-glamour fixes often buy the biggest reliability gains."
