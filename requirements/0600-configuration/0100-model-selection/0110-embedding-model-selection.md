# 0110 Embedding Model Selection

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can choose which sentence-transformer model to use for embeddings, with a sensible default.

## Key Fields/Parameters

- config key: `embedding.model` in `static-agent.toml`
- CLI flag: `--model <huggingface-model-id>`
- default: `sentence-transformers/all-MiniLM-L6-v2`
- the same model ID is used by both the CLI indexer and the browser WASM module

## Acceptance Criteria

- The default model works without any configuration.
- Any HuggingFace sentence-transformer model ID can be specified.
- The CLI and WASM module use the same model (enforced via the index metadata).
- If the WASM module detects a model mismatch with the index, it reports an error.

## Evidence

- `tests/cli/test_model_config.rs`

## Linked Tickets

- (none yet)

## Licensing Notes

The default model (`all-MiniLM-L6-v2`) is Apache 2.0 licensed but was trained on MS MARCO data which has non-commercial restrictions. Models are fetched from HuggingFace CDN at runtime (not redistributed by this project). Users concerned about training data provenance should consider alternatives:

- `BAAI/bge-small-en-v1.5` — MIT license, 33M params
- `Snowflake/snowflake-arctic-embed-s` — Apache 2.0, 33M params
- `nomic-ai/modernbert-embed-base` — Apache 2.0
