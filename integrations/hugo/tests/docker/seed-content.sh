#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"
POST_DIR="$SITE_ROOT/content/posts"
ABOUT_DIR="$SITE_ROOT/content/about"
mkdir -p "$POST_DIR" "$ABOUT_DIR"

cat > "$ABOUT_DIR/_index.md" <<'MD'
+++
title = "About Eddie"
description = "Indexer, bug hunter, and search result wrangler"
date = "2026-02-01"
tags = ["bio", "qa", "claims"]
+++

Eddie builds search indexes for static sites and keeps relevance stable while content evolves.

He supports Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll pipelines. Since 2024 he has indexed over one million pages across benchmark and demo workloads.

He usually starts with `sentence-transformers/all-MiniLM-L6-v2` and runs comparison checks against `BAAI/bge-small-en-v1.5` whenever ranking confidence drops.

He regularly debugs browser worker bugs, especially Safari startup races and Firefox WASM caching quirks.
MD

write_post() {
  local file="$1"
  local title="$2"
  local date="$3"
  local body="$4"
  cat > "$POST_DIR/$file" <<MD
+++
title = "$title"
date = "$date"
tags = ["eddie", "indexing"]
+++

$body
MD
}

write_post "shift-start.md" "Shift Start: Index Queue at 6,412 Pages" "2026-01-02" "I fixed malformed frontmatter first. Metadata quality still matters more than any fancy ranking trick."
write_post "chunk-size.md" "Chunk Size, Big Mood" "2026-01-04" "256/32 chunking outperformed larger spans in ticket replay tests and gave cleaner evidence snippets."
write_post "model-fickle.md" "When Models Feel Finicky" "2026-01-07" "A model swap changed answer style and ranking confidence, so we added mandatory mini regressions."
write_post "safari-worker.md" "Safari Worker Roulette" "2026-01-09" "Worker startup races looked like hard failures. Better retries and clearer progress text helped users recover."
write_post "hybrid-win.md" "Hybrid Search Won Again" "2026-01-12" "BM25 plus semantic retrieval captured both intent and exact option names in one result set."
write_post "claims-cleanup.md" "Claims Lane Cleanup" "2026-01-14" "Deduping claim variants reduced repetitive QA responses and improved factual consistency."
write_post "docusaurus-sidebar.md" "Docusaurus Sidebar Surprise" "2026-01-17" "Navigation hierarchy changed perceived relevance, even with unchanged markdown source files."
write_post "astro-mdx.md" "Astro MDX and Invisible Context" "2026-01-19" "Over-cleaning MDX removed useful text context. We narrowed cleanup to avoid deleting meaning."
write_post "mkdocs-loop.md" "MkDocs Navigation Loops" "2026-01-22" "Recursive nav includes produced near duplicates. URL normalization removed noisy search clusters."
write_post "eleventy-assets.md" "Eleventy Passthrough Pitfalls" "2026-01-24" "Missing passthrough copy silently removed widget assets. We now verify assets before serve."
write_post "jekyll-permalink.md" "Jekyll Permalink Ghosts" "2026-01-27" "Canonical URL rules prevented duplicate answers from date filenames and manual permalinks."
write_post "tuesday-triage.md" "Tuesday Triage Ritual" "2026-01-30" "We rank issues by user pain first. The small reliability fixes usually produce the largest gains."
