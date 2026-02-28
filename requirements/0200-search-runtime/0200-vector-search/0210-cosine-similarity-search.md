# 0210 Cosine Similarity Search

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the WASM module, I compute cosine similarity between the query vector and all index vectors to find the most relevant chunks.

## Key Fields/Parameters

- algorithm: brute-force cosine similarity (sufficient for <10k chunks)
- returns: top-k results (default k=5, configurable)
- computation: runs in a Web Worker to avoid blocking UI

## Acceptance Criteria

- Results are sorted by descending similarity score.
- Search completes in under 100ms for corpora up to 5,000 chunks.
- Search runs in a Web Worker, not on the main thread.

## Evidence

- `tests/wasm/test_search.rs`

## Linked Tickets

- (none yet)
