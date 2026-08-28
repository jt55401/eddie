# 0220 Hybrid Fusion, Result Ranking, and Snippets

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see one ranked result list that combines keyword,
learned-sparse, and dense signals, each with a page title, a text snippet,
and a clickable link.

## Key Fields/Parameters

- fusion: weighted reciprocal rank fusion, `k = 60`; default per-arm weights are dense 1.0, sparse 1.0, BM25 0.8 (BM25 rises to 1.0 when the index has no sparse arm); ties break deterministically by chunk index
- page grouping: best-scoring chunk per URL represents the page; a second distinct-granularity chunk that also scored well on the same page adds a bounded agreement bonus (`+0.10 × its score`)
- recency: a tie-breaker only (`1e-6 × decay`), never a ranking multiplier; undated pages are neutral
- per result: `{ title, url, section, snippet, score, chunks }`
- snippet: the sentence window (≤ 180 chars) with the most query-term hits, taken from the chunk text with its overlap prefix removed, falling back to the chunk start

## Acceptance Criteria

- Each result includes the page title, URL, section heading, and a text snippet.
- Results link directly to the source page on the site.
- Duplicate pages are deduplicated to their best-scoring chunk; the response also lists that page's other matching chunk ids for expansion.
- Fusion, page grouping, and snippet extraction are the same functions (`retrieve`, `group_pages`, `snippet`) used by the CLI (`eddie search`), the WASM runtime, and `eddie eval`/`eddie tune`, so rankings never diverge between them.
- The response reports which arms actually ran (`arms: {dense, sparse, bm25}`) and which, if any, were skipped (`degraded`), so the UI can show a "keyword-only results" notice when appropriate.

## Evidence

- `tests/wasm/test_result_format.rs`
- `tests/cli/test_hybrid_fusion.rs`

## Linked Tickets

- (none yet)
