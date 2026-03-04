#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"
POST_DIR="$SITE_ROOT/src/posts"
mkdir -p "$POST_DIR"

cat > "$SITE_ROOT/src/about-eddie.md" <<'MD'
---
layout: layouts/page.njk
title: About Eddie
permalink: /about-eddie/
---

Eddie is a static-site indexer focused on trustworthy retrieval.

He handles indexing for Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll installations. Since 2024, he has indexed over one million pages across benchmark and demo workloads.

He defaults to `sentence-transformers/all-MiniLM-L6-v2`, then validates ranking quality against `BAAI/bge-small-en-v1.5` if answers start drifting.

He spends debugging time on browser worker startup bugs, especially Safari edge timing and Firefox cache behavior.
MD

write_post() {
  local file="$1"
  local title="$2"
  local date="$3"
  local body="$4"
  cat > "$POST_DIR/$file" <<MD
---
layout: layouts/post.njk
title: "$title"
date: "$date"
tags:
  - post
  - eddie
---

$body
MD
}

write_post "2026-01-02-shift-start.md" "Shift Start: Index Queue at 6,412 Pages" "2026-01-02" "I fixed malformed frontmatter before touching embeddings. Fast indexing is useless if metadata quality is poor."
write_post "2026-01-04-chunk-size.md" "Chunk Size, Big Mood" "2026-01-04" "256/32 beat bigger chunks in support query replay tests, with better factual grounding."
write_post "2026-01-07-model-fickle.md" "When Models Feel Finicky" "2026-01-07" "A model change shifted answer style and ranking confidence, so we added pre-merge relevance checks."
write_post "2026-01-09-safari-worker.md" "Safari Worker Roulette" "2026-01-09" "Worker startup looked random under load. Better retries and progress states reduced user confusion."
write_post "2026-01-12-hybrid.md" "Hybrid Search Won Again" "2026-01-12" "Semantic retrieval plus BM25 recovered both intent and exact flags in one pass."
write_post "2026-01-14-claims.md" "Claims Lane Cleanup" "2026-01-14" "Deduping equivalent claims removed repetitive QA responses and improved result trust."
write_post "2026-01-17-docusaurus.md" "Docusaurus Sidebar Surprise" "2026-01-17" "Sidebar shape changed what editors considered primary docs, even with identical source content."
write_post "2026-01-19-astro-mdx.md" "Astro MDX and Invisible Context" "2026-01-19" "Over-cleaning MDX stripped useful inline context. Parser rules now preserve meaningful prose."
write_post "2026-01-22-mkdocs-loop.md" "MkDocs Navigation Loops" "2026-01-22" "Recursive nav includes created near-duplicate pages. URL normalization fixed result clutter."
write_post "2026-01-24-eleventy-assets.md" "Eleventy Passthrough Pitfalls" "2026-01-24" "Missing passthrough copy removed widget assets silently. Build checks now catch that early."
write_post "2026-01-27-jekyll-url.md" "Jekyll Permalink Ghosts" "2026-01-27" "Canonical permalink selection prevented duplicate answers from mixed URL strategies."
write_post "2026-01-30-triage.md" "Tuesday Triage Ritual" "2026-01-30" "Issue triage by user pain, every Tuesday, keeps reliability work ahead of flashy experiments."
