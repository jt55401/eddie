# 0320 Index Serialization

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the CLI indexer, I serialize chunks, embeddings, and metadata into a compact binary file that the WASM module can load efficiently.

## Key Fields/Parameters

- output format: custom binary (header + metadata JSON + embedding matrix)
- file: `static-agent-index.bin` (configurable)
- contains: chunk texts, per-chunk metadata, embedding vectors, model ID

## Acceptance Criteria

- The index file is self-describing (includes model ID and vector dimensions).
- The WASM module can memory-map or stream-load the index.
- Index size scales linearly with corpus size (no bloat).
- The format is versioned for forward compatibility.

## Evidence

- `tests/cli/test_serialization.rs`
- `tests/wasm/test_index_loading.rs`

## Linked Tickets

- (none yet)
