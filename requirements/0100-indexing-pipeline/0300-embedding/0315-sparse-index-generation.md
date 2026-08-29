# 0315 Learned Sparse Index Generation

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the CLI indexer, I build a learned sparse retrieval arm at index time so
the browser can score keyword-like relevance without downloading or running
any model at query time.

## Key Fields/Parameters

- `--sparse`: enable the sparse arm with its default model (`opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill`)
- `--sparse-model <huggingface-id>`: enable the sparse arm with an explicit model (mutually exclusive with `--sparse`)
- doc-side scoring: `w(t) = max_over_positions(log(1 + relu(logit_t)))`, pruned to terms with weight ≥ 0.1 × the chunk's max weight
- the index stores, per term that actually occurs in the postings: the token id, its IDF, and per-document weight (fixed-point, `u16`); plus the tokenizer's HuggingFace repo id and a SHA-256 of `tokenizer.json` so the browser runtime can fetch and validate a matching tokenizer
- query side needs no model: WordPiece-tokenize the query, look up each token's stored IDF, collapse duplicate tokens to their max weight, and score `Σ q_w · d_w` — this logic is shared between the native CLI and the WASM runtime

## Acceptance Criteria

- An index built with `--sparse` (or `--sparse-model`) contains a `sparse` payload section with per-term postings and IDF.
- The sparse arm requires no model download or forward pass at query time in the browser, only the (~700 KB) tokenizer.
- An index built without `--sparse`/`--sparse-model` has no `sparse` section, and the runtime falls back to BM25 weight 1.0 (dense arm still runs if present).
- Query-side term weighting matches between `eddie search --mode sparse` and the WASM `search_with` sparse path for identical index and query.

## Evidence

- `tests/cli/test_sparse_index.rs`

## Linked Tickets

- (none yet)
