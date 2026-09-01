# 0220 Hybrid Fusion, Result Ranking, and Snippets

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see one ranked result list that combines keyword,
learned-sparse, and dense signals, each with a page title, a text snippet,
and a clickable link.

## Key Fields/Parameters

- fusion: weighted reciprocal rank fusion, `k = 60`; default per-arm weights are dense 1.0, sparse 1.0, BM25 0.8 (BM25 rises to 1.0 when the index has no sparse arm); ties break deterministically by chunk index
- page grouping: best-scoring chunk per URL represents the page; a second distinct-granularity chunk that also scored well on the same page adds a bounded agreement bonus (`+0.10 × its score`)
- recency: on a **browse-style** query only, the page score is multiplied by `1 + strength × 0.5^(age_days / half_life_days)`, with `strength` and `half_life_days` from `manifest.recency` (`eddie index --recency` / `--recency-half-life`, default 0.15 and 1460 days, `--no-recency` to omit). Ages are measured from the newest date in the corpus, not the clock, so an index ranks the same way whenever it is searched; an undated or unparseable page is never moved, and the boost only ever lifts. Date also remains a sort tie-breaker, newest first
- query kind: `search::looks_like_question` (a question mark, an opening question word, or ≥ 5 words -- the same rule the widget uses for the FAQ card) decides whether the recency boost applies at all. A question gets none of it. This reverses the earlier "tie-breaker only, never a ranking multiplier" rule, which was right for questions and wrong for browse queries; see `docs/reviews/2026-09-01-recency-boost.md` for the measurements that forced the split
- per result: `{ title, url, section, snippet, score, chunks }`
- snippet: the sentence window (≤ 180 chars) with the most query-term hits, taken from the chunk text with its overlap prefix removed, falling back to the chunk start

## Acceptance Criteria

- A question-style query ranks identically whether or not the index carries a recency spec: the boost is gated off for it (measured: 45 labelled questions score MRR 0.814 / nDCG@10 0.774 at every strength from 0 to 0.6).
- A browse-style query on an index with a recency spec demotes stale matches without overruling topical ones: on jason-grey.com, `java` keeps the 2011 post that is about a Java API in first place while the 2009 and 2006 posts that merely mention Java leave the top five.
- Each result includes the page title, URL, section heading, and a text snippet.
- Results link directly to the source page on the site.
- Duplicate pages are deduplicated to their best-scoring chunk; the response also lists that page's other matching chunk ids for expansion.
- Fusion, page grouping, and snippet extraction are the same functions (`retrieve`, `group_pages`, `snippet`) used by the CLI (`eddie search`), the WASM runtime, and `eddie eval`/`eddie tune`, so rankings never diverge between them.
- The response reports which arms actually ran (`arms: {dense, sparse, bm25}`) and which, if any, were skipped (`degraded`), so the UI can show a "keyword-only results" notice when appropriate.

## Evidence

- `tests/wasm/test_result_format.rs`
- `docs/reviews/2026-09-01-recency-boost.md` — the sweeps, the per-query diffs and why the boost is gated on query kind
- `tests/cli/test_hybrid_fusion.rs`

## Linked Tickets

- (none yet)
