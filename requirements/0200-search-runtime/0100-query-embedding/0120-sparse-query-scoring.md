# 0120 Sparse Query Scoring

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the browser runtime, I score the learned sparse arm against the user's
query without downloading or running any model.

## Key Fields/Parameters

- fetches the sparse tokenizer (~700 KB) the same way as a model file, then validates it against the `vocab_hash` recorded in the index manifest
- tokenizes the query with that tokenizer, looks up each token's IDF from the index's stored per-term weights, and collapses duplicate tokens to their max weight
- scoring (`Σ q_w · d_w`) is shared code between the native CLI and the WASM runtime (`sparse_query_terms`), so the two never disagree on a given index and query

## Acceptance Criteria

- No model download or forward pass is required to score the sparse arm — only the tokenizer.
- A tokenizer hash mismatch is reported once and the sparse arm is skipped for that session; BM25 and dense (if available) still run.
- `eddie search --mode sparse` and the WASM sparse path produce identical rankings for the same index and query.

## Evidence

- `tests/wasm/test_sparse_query_scoring.rs`

## Linked Tickets

- (none yet)
