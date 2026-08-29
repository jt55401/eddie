# 0110 Query Embedding (Dense Lane Selection)

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the browser runtime, I pick the best dense lane the visitor's device can
run, download that lane's model, and embed the user's search query into a
vector that scores against the same lane's stored vectors.

## Key Fields/Parameters

- lane selection order: `webgpu-onnx` (via transformers.js) when `navigator.gpu` gives an adapter and the site allows it; else the first `wasm-candle` lane; else dense is skipped entirely (BM25 + sparse still run)
- `wasm-candle` lane: Candle compiled to `wasm32-unknown-unknown`, `tokenizers` crate (WASM-compatible), model files fetched from `https://huggingface.co/<repo>/resolve/<revision>/<file>` with the revision pinned in the manifest
- `webgpu-onnx` lane: `pipeline("feature-extraction", repo, { device: "webgpu", dtype })`, `dtype` is the `f16` variant only when `adapter.features.has("shader-f16")`, else an `f32` fallback
- caching: model files cached in IndexedDB keyed by `repo@revision/file`; a cache failure only logs, it never blocks search
- fetches use an `AbortController` timeout with one retry

## Acceptance Criteria

- The embedding model is not bundled — it is fetched from HuggingFace at runtime, pinned to the revision recorded in the index manifest.
- Model download progress is reported (`loading_model {file, progress}`), falling back to a spinner when `Content-Length` is missing.
- After first download, the model is cached and subsequent loads are instant.
- Query embeddings for a given lane match the CLI indexer's embeddings for identical input and lane.
- If no dense lane is runnable, the widget reports it once and still serves BM25 + sparse results — dense is never a hard requirement for search to work.

## Evidence

- `tests/wasm/test_query_embedding.rs`

## Linked Tickets

- (none yet)
