<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Eddie browser runtime

`widget/build.sh` produces five files in `dist/`:

| File | Role |
|---|---|
| `eddie-widget.js` | Search UI (closed Shadow DOM). Reads the `data-*` attributes on its `<script>` tag. |
| `eddie-worker.js` | Classic Web Worker: loads the index, the WASM retriever and the dense model; answers `search`/`page`/`chunk`/`qa`. |
| `eddie-agent-worker.js` | Module worker created on the first "Ask": runs WebLLM and streams a cited answer over evidence the widget hands it. |
| `eddie-wasm.js`, `eddie.wasm` | wasm-bindgen glue + retriever (`src/wasm.rs`). |

The three JS entry points are built by concatenating `widget/src/lib/*.js`
(pure helpers, exposed as `EddieLib`) with their main file. There is no
bundler; edit `widget/src/**` and rerun `bash widget/build.sh` (or
`bash widget/build.sh --js-only` to skip the WASM build and only
reassemble the bundles from an earlier `widget/pkg/`).

## Tests

```bash
node --test widget/test/*.test.js
```

The tests cover the pure modules only (URL and version handling, lane
selection, download sizes and consent copy, streaming downloads with retry
and SHA-256 verification, model id selection, think-stripping, plan parsing,
evidence merging, citation post-processing).

## Script tag

```html
<script src="/eddie/eddie-widget.js?v=<build hash>"
        data-index-url="/eddie/index.ed?v=<build hash>"
        data-position="bottom-right"     <!-- top-left | top-right | bottom-left | bottom-right -->
        data-theme="auto"                <!-- auto | light | dark -->
        data-offset-x="0" data-offset-y="0"
        data-qa-mode="auto"              <!-- off | auto | always: show a "From the FAQ" card from qa_lookup hits -->
        data-qa-subject=""               <!-- site description used in the agent prompts; defaults to the hostname -->
        data-top-k="8"                   <!-- results per search -->
        data-answer-top-k="5"            <!-- qa_lookup hits requested (capped at 3 shown) -->
        data-agent-mode="auto"           <!-- off | auto -->
        data-agent-model="auto"          <!-- auto | quality | <WebLLM model id> -->
        data-dense-runtime="auto"        <!-- auto | wasm | webgpu -->
        data-consent-text=""             <!-- override of the download consent copy; {size} and {model} are substituted -->
        defer></script>
```

`?v=` on the script `src` (or, failing that, on `data-index-url`) is also
appended to `eddie-worker.js`, `eddie-wasm.js`, `eddie.wasm` and
`eddie-agent-worker.js` so a redeploy never pairs cached glue with a new
binary. When `data-index-url` is absent the index is `index.ed` next to the
widget script.

## Host element

The widget mounts as `<div id="eddie-host">` (closed Shadow DOM) and mirrors
its state on that element for page CSS and tests:

| Attribute | Values |
|---|---|
| `data-theme` | `auto`, `light`, `dark` (from `data-theme` on the script tag) |
| `data-state` | `idle`, `loading`, `index_ready`, `awaiting_consent`, `ready`, `error`, `dead` |
| `data-lane` / `data-runtime` | dense lane id and `wasm` or `webgpu` once ready (empty when no lane loaded) |
| `data-arms` | comma-separated arms in use once ready, e.g. `dense,sparse,bm25` |

## What happens at runtime

1. The worker fetches the index, reads its manifest (`wasm_bindgen.manifest`)
   and parses it (`init_index`). Keyword (BM25) search works from this point.
2. Dense lane: `data-dense-runtime="auto"` picks a `webgpu-onnx` lane when
   `navigator.gpu` yields an adapter, else the first `wasm-candle` lane, else
   none. Before the first download the widget shows a consent card with the
   lane's approximate size (skipped when the files are already in IndexedDB;
   `navigator.connection.saveData` is mentioned in the copy). Declining keeps
   keyword-only search and says so.
3. Model files come from `https://huggingface.co/<repo>/resolve/<revision>/<file>`
   with the revision pinned by the manifest, a 60 s timeout per file (10 min
   for weights), one retry with backoff, and an IndexedDB cache keyed
   `repo@revision/file`. A cache failure only logs.
4. Sparse lane: the manifest's `tokenizer.json` is fetched the same way and
   its SHA-256 is checked against `vocab_hash` before `init_sparse_tokenizer`.
5. WebGPU lanes import transformers.js from jsDelivr and embed queries in the
   worker; the vector is passed to `search` together with the lane id. Its
   ONNX files go into the same IndexedDB store (`env.customCache`), keyed
   like the wasm lane files.
6. A lane that fails is reported once in `degraded`; search continues with
   the remaining arms and the widget shows a "keyword-only" notice. A
   wasm-candle lane the WASM loader cannot run (non-BERT family, sharded
   safetensors or `pytorch_model.bin` weights) is skipped with a note and
   the next lane is tried. A WebGPU lane whose vector is non-finite degrades
   that query to the other arms. A trapped
   WASM panic is fatal: the worker reports it and the widget offers Retry,
   which starts a fresh worker.
7. "Ask" (button or Shift+Enter) appears only when `data-agent-mode` is not
   `off` and a WebGPU adapter exists. The first click asks for consent
   (0.8B ≈ 0.4 GB, 2B ≈ 1.2 GB; remembered in `localStorage`
   `eddie.agent.consent`), loads WebLLM from esm.run in a module worker,
   plans 1–3 queries (JSON-schema constrained), runs them plus the raw
   question through the search worker (hybrid, top 6), merges up to 6
   evidence chunks, and streams the answer with `[n]` citations. Typing a
   new query, Stop or Esc aborts generation. Data Saver disables the agent.

## Worker protocol

Main thread → `eddie-worker.js`:

| Message | Fields |
|---|---|
| `init` | `indexUrl`, `baseUrl`, `version?`, `denseRuntime?`, `consent?` (re-sending `init` resumes after consent or retries after an error) |
| `cache_check` | `requestId` |
| `search` | `requestId`, `query`, `topK?`, `mode?` (`hybrid`/`dense`/`sparse`/`keyword`), `evidence?` (attach best chunk text), `qa?` (k) |
| `page` / `chunk` / `qa` | `requestId` + `url` / `id` / `query`, `k?` |

Worker → main thread:

| Message | Fields |
|---|---|
| `status` | `state`: `loading_wasm`, `loading_index {progress}`, `index_ready {manifest, lanes}`, `consent_required {lane, sizeBytes, saveData}`, `downloading_model {file, progress, loaded, total}`, `loading_model {lane}`, `error {message, fatal, unsupported}` |
| `ready` | `lanes`, `lane`, `runtime` (`wasm`/`webgpu`), `arms {dense, sparse, bm25}`, `degraded[]`, `manifest` |
| `cache_result` | `requestId`, `cached`, `lane`, `sizeBytes` |
| `search_result` | `requestId`, `results[]` (PageResult, plus `text` when `evidence`), `arms`, `degraded[]`, `mode`, `lane`, `qa?` |
| `page_result` / `chunk_result` / `qa_result` | `requestId` + `page` / `chunk` / `hits` |
| `error` | `requestId?`, `message`, `fatal`, `unsupported` |

Main thread → `eddie-agent-worker.js`: `load {model}`, `plan {requestId, question, site}`,
`ask {requestId, question, site, evidence: [{title, url, text}]}`, `abort {requestId?}`.
Back: `progress {text, progress}`, `loaded {model, loadMs}`, `plan_result {requestId, queries}`,
`token {requestId, text}`, `done {requestId, answer, citations: [{n, url, title}], nohit, usage}`,
`aborted {requestId}`, `error {requestId?, message}`.

## Content-Security-Policy

Nothing is bundled from a CDN; the libraries load on demand. A site with a
CSP needs:

```
script-src  'self' https://cdn.jsdelivr.net https://esm.run;
worker-src  'self' blob:;
connect-src 'self' https://huggingface.co https://*.hf.co https://cdn.jsdelivr.net https://esm.run https://raw.githubusercontent.com;
```

- `cdn.jsdelivr.net`: transformers.js (WebGPU dense lanes only).
- `esm.run` (and its jsDelivr redirects): WebLLM (agent only).
- `huggingface.co` / `*.hf.co`: model files (the resolve endpoint redirects to `cdn-lfs*.hf.co` / `*.aws.cdn.hf.co`).
- `raw.githubusercontent.com`: WebLLM's model-library WASM files, and `blob:` workers for WebLLM's internal use.
- Without the agent and without WebGPU lanes, `script-src 'self'` and `connect-src 'self' https://huggingface.co https://*.hf.co` are enough.
