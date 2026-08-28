# 0310 Dense Embedding Generation

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the CLI indexer, I run one or more dense embedding models against each
chunk to produce dense vector lanes, so the browser runtime can pick
whichever lane its device can run.

## Key Fields/Parameters

- models: `--dense-model <huggingface-id>`, repeatable — each becomes its own lane in the index; default is `sentence-transformers/multi-qa-MiniLM-L6-cos-v1`
- `--preset fast|balanced|quality|gpu` bundles a dense model set (and sparse arm, and device) in one flag; see [0110 Embedding Model Selection](../../0600-configuration/0100-model-selection/0110-embedding-model-selection.md) for the table
- `--device auto|cpu|cuda` and `--batch-size` (default `32`)
- supported architectures: bert (Candle `bert`), xlm-roberta (bge-m3), qwen3 (Qwen3-Embedding, Harrier) — all via Candle at index time
- runtime lane assignment: bert-family models run as `wasm-candle` lanes (CPU, in the browser's WASM module); xlm-roberta and qwen3 models run as `webgpu-onnx` lanes (via transformers.js, browser-side only, not usable natively at index time by the browser)
- output: one float32 (or int8-quantized) vector per chunk per lane
- model source: downloaded from HuggingFace Hub on first run, cached locally

## Acceptance Criteria

- Embeddings are deterministic for the same input and model.
- Model weights are fetched from HuggingFace Hub (not bundled in the repo).
- `--dense-model` may be repeated to build more than one lane in a single index.
- An index built with `--device cuda` requires a binary built with `--features cuda` (not part of the published release binaries — build locally against a CUDA toolchain).
- Progress and per-lane truncation counts are reported during embedding generation.
- ModernBERT-family models (`nomic-ai/modernbert-embed-base`) are not supported — Candle's `bert` loader is architecturally incompatible with ModernBERT's fused-QKV, rotary-attention layout, so this model must not be recommended anywhere in docs.

## Evidence

- `tests/cli/test_embedding.rs`

## Linked Tickets

- (none yet)
