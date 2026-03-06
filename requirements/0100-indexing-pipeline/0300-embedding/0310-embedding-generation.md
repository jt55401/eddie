# 0310 Embedding Generation

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the CLI indexer, I run a sentence-transformer model against each chunk to produce a dense vector embedding.

## Key Fields/Parameters

- model: configurable via `--model <name-or-path>` (default: `multi-qa-MiniLM-L6-cos-v1`)
- runtime: Candle (Rust) with safetensors weights
- output: one float32 vector per chunk (384-dim for MiniLM)
- model source: downloaded from HuggingFace Hub on first run, cached locally

## Acceptance Criteria

- Embeddings are deterministic for the same input and model.
- Model weights are fetched from HuggingFace Hub (not bundled in the repo).
- The model is configurable; any ONNX-compatible sentence-transformer works.
- Progress is reported during model download and embedding generation.

## Evidence

- `tests/cli/test_embedding.rs`

## Linked Tickets

- (none yet)
