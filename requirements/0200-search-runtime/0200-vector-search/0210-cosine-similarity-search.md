# 0210 Dense Lane Top-K Scoring

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the WASM module, I score the query vector against one dense lane's
stored vectors to find that lane's most relevant chunks.

## Key Fields/Parameters

- algorithm: brute-force cosine similarity on L2-normalized vectors (sufficient for the corpus sizes Eddie targets, typically under 10k chunks); vectors may be stored `f32` or `int8`-quantized with a per-row scale
- returns: top-k results per arm (`fetch_k = max(3 × top_k, 30)`) before fusion, not the final top-k shown to the user
- computation: runs in a Web Worker to avoid blocking the UI thread

## Acceptance Criteria

- Results from one dense lane are sorted by descending similarity score.
- Dense scoring for a single lane completes in under 100ms for corpora up to 5,000 chunks.
- Search runs in a Web Worker, not on the main thread.
- int8-quantized lanes produce rankings equivalent to their f32 source within a documented tolerance.

## Evidence

- `tests/wasm/test_search.rs`

## Linked Tickets

- (none yet)
