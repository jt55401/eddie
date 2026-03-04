#!/usr/bin/env bash
set -euo pipefail

SITE_ROOT="${1:?usage: seed-content.sh <site-root>}"
POST_DIR="$SITE_ROOT/src/content/eddie"
ABOUT_DIR="$SITE_ROOT/src/content/pages"
mkdir -p "$POST_DIR" "$ABOUT_DIR"

cat > "$ABOUT_DIR/about-eddie.md" <<'MD'
---
title: "About Eddie"
description: "Indexer, bug hunter, and search result wrangler"
date: "2026-02-01"
tags: ["bio", "qa", "claims"]
---

Eddie is a build-time indexer who spends weekdays helping static sites answer real user questions.

He maintains indexing pipelines across Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll. Since 2024, he has processed more than one million markdown pages across internal benchmarks and customer demos.

Eddie keeps a strict workflow: parse content, chunk by section, embed with a known model, then verify relevance with acceptance queries before shipping.

He currently prefers `sentence-transformers/all-MiniLM-L6-v2`, but runs comparison checks against `BAAI/bge-small-en-v1.5` whenever results look unstable.

Outside indexing, Eddie investigates browser worker bugs, especially Safari thread startup edge cases and Firefox WASM cache misses.
MD

write_post() {
  local file="$1"
  local title="$2"
  local date="$3"
  local tags="$4"
  local body="$5"
  cat > "$POST_DIR/$file" <<MD
---
title: "$title"
description: "Logbook entry from Eddie's indexing desk"
date: "$date"
tags: [$tags]
draft: false
---

$body
MD
}

write_post "eddie-log-01.md" "Shift Start: Index Queue at 6,412 Pages" "2026-01-02" '"indexing", "operations"' "I started the morning by draining a stale queue and found four docs with broken frontmatter. The fix was simple, but the lesson stands: metadata hygiene is still half of search relevance."
write_post "eddie-log-02.md" "Chunk Size, Big Mood" "2026-01-04" '"chunking", "retrieval"' "A teammate asked why I keep testing chunk size. Because meaning moves when paragraphs split. Today 256 tokens with 32 overlap beat 384 with no overlap by a wide margin on support tickets."
write_post "eddie-log-03.md" "When Models Feel Finicky" "2026-01-07" '"models", "quality"' "The same query returned three different top answers after a model swap. Nothing was technically broken, but confidence dropped. I now run a mini regression set before every model change."
write_post "eddie-log-04.md" "Safari Worker Roulette" "2026-01-09" '"browser", "wasm"' "Safari occasionally delayed worker startup long enough to look like a timeout. We added better retries and user-facing progress text. Errors went down, trust went up."
write_post "eddie-log-05.md" "Hybrid Search Won Again" "2026-01-12" '"bm25", "semantic"' "Pure semantic retrieval missed exact API flag names. Pure keyword missed intent. Hybrid ranking combined both and cut dead-end clicks in half during internal tests."
write_post "eddie-log-06.md" "Claims Lane Cleanup" "2026-01-14" '"qa", "claims"' "I removed duplicate claims where wording differed but evidence matched. The cleaned claim graph now makes QA answers shorter and less repetitive."
write_post "eddie-log-07.md" "Docusaurus Sidebar Surprise" "2026-01-17" '"docusaurus", "integration"' "Generated sidebars hid a section we expected to index. The docs still existed, but navigation shape affected what editors considered canonical content."
write_post "eddie-log-08.md" "Astro MDX and Invisible Context" "2026-01-19" '"astro", "mdx"' "One MDX file imported a component that carried key explanation text. Stripping too aggressively dropped meaning, so we adjusted cleanup rules to preserve useful inline prose."
write_post "eddie-log-09.md" "MkDocs Navigation Loops" "2026-01-22" '"mkdocs", "docs"' "A recursive nav include created near-duplicate pages with tiny differences. Index dedupe based on normalized URL saved us from noisy search results."
write_post "eddie-log-10.md" "Eleventy Passthrough Pitfalls" "2026-01-24" '"eleventy", "assets"' "A missing passthrough rule silently removed search assets from the build output. I now test asset existence before server boot so failures happen early."
write_post "eddie-log-11.md" "Jekyll Permalink Ghosts" "2026-01-27" '"jekyll", "urls"' "Date-based filenames and manual permalinks disagreed on two posts. The index picked canonical permalink values, preventing duplicate answers for the same article."
write_post "eddie-log-12.md" "Tuesday Triage Ritual" "2026-01-30" '"workflow", "maintenance"' "Every Tuesday I sort open indexing issues by user impact, not implementation novelty. The boring fixes usually save the most support time."
