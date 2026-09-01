# 0110 Query Embedding (Dense Lane Selection)

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the browser runtime, I pick the best dense lane the visitor's device can
run, download that lane's model, and embed the user's search query into a
vector that scores against the same lane's stored vectors.

## Key Fields/Parameters

- WASM module split: `eddie-lite.wasm` (index parsing, BM25, the learned sparse arm with its query-side tokenizer, RRF, snippets, QA ranking, sidecars — no embedding-model code at all) is what every visitor who opens search loads; `eddie-dense.wasm` (lite plus the Candle BERT embedder for `wasm-candle` lanes) is fetched only after a visitor accepts a CPU dense lane, and the loaded index is handed over to it (`init_index` again, `attach_sidecar` per sidecar, `init_sparse_tokenizer` if the vocabulary was fetched separately) rather than reloaded from scratch — see [0400-widget-ui's persistent runtime story](../../0400-widget-ui/0500-persistent-runtime/0510-tiered-service-workers.md) for how the two modules are hosted
- lane selection order: `webgpu-onnx` (via transformers.js) when `navigator.gpu` gives an adapter and the site allows it; else the first `wasm-candle` lane (which requires `eddie-dense.wasm`); else dense is skipped entirely (BM25 + sparse still run on `eddie-lite.wasm` alone)
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
- A visitor who never accepts a `wasm-candle` lane never downloads `eddie-dense.wasm` or any embedding-model code — the base module every search opens with (`eddie-lite.wasm`) contains no embedder.
- Accepting a `wasm-candle` lane after search is already running does not re-fetch or re-parse the index; the already-loaded index and sidecars are handed to `eddie-dense.wasm`.

## Evidence

- `tests/wasm/test_query_embedding.rs`
- `widget/build.sh` (lite/dense WASM build split)

## Linked Tickets

- (none yet)
