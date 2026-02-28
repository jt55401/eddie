# 0110 Query Embedding in WASM

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the browser WASM module, I download the same sentence-transformer model used at index time and embed the user's search query into a vector.

## Key Fields/Parameters

- model: fetched from HuggingFace CDN on first use
- runtime: Candle compiled to `wasm32-unknown-unknown`
- tokenizer: HuggingFace `tokenizers` crate (WASM-compatible)
- caching: model weights cached in browser (IndexedDB or Cache API)

## Acceptance Criteria

- The embedding model is not bundled — it is fetched from HuggingFace CDN at runtime.
- Model download shows progress to the user (bytes received / total).
- After first download, the model is cached and subsequent loads are instant.
- Query embeddings produce the same vectors as the CLI indexer for identical input.

## Evidence

- `tests/wasm/test_query_embedding.rs`

## Linked Tickets

- (none yet)
