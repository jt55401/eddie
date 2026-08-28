# 0320 Index Serialization (Format v5)

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the CLI indexer, I serialize chunks, all three retrieval arms, and
metadata into a compact binary file that the WASM module can load and
validate without trusting the byte stream.

## Key Fields/Parameters

- container: `SAED` v2 — `"SAED" | u32 version | u32 manifest_len | manifest JSON | u32 payload_len | u32 payload_crc32 | u32 decompressed_len | brotli(payload)`; the manifest is readable without decompressing anything, so the runtime can decide which models to fetch first
- payload: `SAGI` v5 — a sequence of `u32 name_len | name | u32 body_len | body` sections (`meta`, `texts`, `bm25`, `sparse`, one `dense/<scope>/<lane_id>` per dense lane, optionally `qa`/`claims`); unknown section names are skipped for forward compatibility
- manifest fields: format version, chunk/page counts, per-lane dense specs (model id, family, dim, pooling, quantization, runtime kind), sparse config, BM25 params, built-at timestamp
- file: `.ed` (default `index.ed`, path set via `--output`)

## Acceptance Criteria

- The index file is self-describing: the manifest lists every dense lane's model id, dimensions, and runtime kind, and the sparse config's model id and tokenizer hash.
- Every length prefix is checked against the remaining bytes before allocation; `decompressed_len` bounds the brotli output; CRC32 covers the decompressed payload — a truncated or corrupted file is rejected with an error, never a panic or silent misread.
- Legacy `SAED` v1 / `SAGI` v4 (and earlier) files are rejected with a "rebuild with eddie 0.4" message; there is no in-place migration.
- `eddie stats --index <path>` and the WASM `manifest()` call both read the manifest without decompressing the payload.

## Evidence

- `tests/cli/test_serialization.rs`
- `tests/wasm/test_index_loading.rs`

## Linked Tickets

- (none yet)
