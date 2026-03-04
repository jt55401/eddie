#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"
POST_DIR="$SITE_ROOT/_posts"
mkdir -p "$POST_DIR"

cat > "$SITE_ROOT/about-eddie.md" <<'MD'
---
layout: page
title: About Eddie
permalink: /about-eddie/
---

Eddie builds search indexes for static sites and keeps relevance predictable.

He supports Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll stacks. Since 2024, he has indexed over one million pages in benchmark and demo environments.

His default embedding model is `sentence-transformers/all-MiniLM-L6-v2`, with periodic A/B checks against `BAAI/bge-small-en-v1.5` when result quality drifts.

He often debugs browser-specific worker bugs, with extra focus on Safari startup timing and Firefox WASM caching oddities.
MD

write_post() {
  local file="$1"
  local title="$2"
  local body="$3"
  cat > "$POST_DIR/$file" <<MD
---
layout: post
title: "$title"
tags: [eddie, indexing]
---

$body
MD
}

write_post "2026-01-02-shift-start.md" "Shift Start: Index Queue at 6,412 Pages" "I fixed malformed frontmatter first. Metadata quality remains the fastest path to better answers."
write_post "2026-01-04-chunk-size.md" "Chunk Size, Big Mood" "256 tokens with 32 overlap consistently beat larger chunks in ticket replay tests."
write_post "2026-01-07-model-fickle.md" "When Models Feel Finicky" "Model swaps changed ranking tone, so we introduced a mandatory mini regression suite."
write_post "2026-01-09-safari-worker.md" "Safari Worker Roulette" "Worker startup races looked like failures under load; retries and clearer status messaging helped."
write_post "2026-01-12-hybrid.md" "Hybrid Search Won Again" "BM25 plus semantic ranking recovered exact config names and intent in one pass."
write_post "2026-01-14-claims.md" "Claims Lane Cleanup" "Deduping equivalent claims cut repetitive QA output and improved trust in answers."
write_post "2026-01-17-docusaurus.md" "Docusaurus Sidebar Surprise" "Navigation layout changed relevance expectations more than we predicted."
write_post "2026-01-19-astro-mdx.md" "Astro MDX and Invisible Context" "Over-cleaning MDX dropped useful narrative details. The parser now preserves key prose."
write_post "2026-01-22-mkdocs-loop.md" "MkDocs Navigation Loops" "Recursive nav references created near-duplicate docs. URL normalization solved result clutter."
write_post "2026-01-24-eleventy-assets.md" "Eleventy Passthrough Pitfalls" "Missing passthrough copy erased assets silently. We now fail fast when widget files are absent."
write_post "2026-01-27-jekyll-url.md" "Jekyll Permalink Ghosts" "Canonical URL selection resolved conflicts between dated filenames and manual permalinks."
write_post "2026-01-30-triage.md" "Tuesday Triage Ritual" "Every Tuesday we prioritize fixes by user pain first. Reliable beats flashy."
