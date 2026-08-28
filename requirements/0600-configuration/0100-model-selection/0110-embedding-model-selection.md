# 0110 Embedding Model Selection

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can choose which dense embedding model(s) to index
with, use a preset instead of picking manually, and choose whether to
enable the learned sparse arm, all with sensible defaults.

## Key Fields/Parameters

- CLI flag: `--dense-model <huggingface-id>`, repeatable (one lane per flag); default `sentence-transformers/multi-qa-MiniLM-L6-cos-v1`
- CLI flag: `--preset fast|balanced|quality|gpu` bundles a dense model set (and sparse, and device) in one flag
- CLI flags: `--sparse` (default sparse model) or `--sparse-model <id>` (explicit)
- CLI flag: `--dense-runtime` is not an indexing-time concept; at query time the widget's `data-dense-runtime` (`auto`, `wasm`, `webgpu`) can force one lane instead of auto-selecting
- there is no config file; the same model id(s) selected at index time are recorded in the index manifest, and the browser reads them from there, so there is never a separate query-time model flag to keep in sync

## Acceptance Criteria

- The default model works without any configuration.
- Any dense model Eddie's embedder supports (bert, xlm-roberta, or qwen3 family) can be specified via `--dense-model`.
- `eddie search` and the WASM runtime read the model id(s), family, and runtime kind from the index manifest; there is no `--model` flag to accidentally mismatch.
- `data-dense-runtime` can force `wasm` or `webgpu` for testing or to avoid a slow lane on a known-weak device; `auto` (default) picks the best runnable lane.

## Evidence

- `tests/cli/test_model_config.rs`

## Linked Tickets

- (none yet)

## Licensing Notes

Models are fetched from HuggingFace at runtime (not redistributed by this
project). See README.md's model table for the full list; in summary:

- `sentence-transformers/multi-qa-MiniLM-L6-cos-v1` (default, `wasm-candle`) — Apache-2.0
- `BAAI/bge-small-en-v1.5` (`wasm-candle`) — MIT
- `Snowflake/snowflake-arctic-embed-s` (`wasm-candle`) — Apache-2.0
- `Qwen/Qwen3-Embedding-0.6B` (`webgpu-onnx`) — Apache-2.0
- `microsoft/harrier-oss-v1-0.6b` (`webgpu-onnx`) — MIT
- `BAAI/bge-m3` (`webgpu-onnx`) — MIT

`nomic-ai/modernbert-embed-base` is not supported: Candle's `bert` loader
cannot load ModernBERT's architecture (fused QKV, rotary attention, no
token-type embeddings). Do not recommend it anywhere in docs.
