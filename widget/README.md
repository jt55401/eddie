<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Eddie browser runtime

`widget/build.sh` produces eight files in `dist/`:

| File | Role |
|---|---|
| `eddie-widget.js` | Search UI (closed Shadow DOM). Reads the `data-*` attributes on its `<script>` tag. |
| `eddie-sw.js` | Module service worker: hosts the search engine and the agent so they survive navigations (see [Persistent engines](#persistent-engines)). |
| `eddie-worker.js` | Classic Web Worker, the page-side fallback for the search engine: loads the index, the WASM retriever and the dense model; answers `search`/`page`/`chunk`/`qa`. |
| `eddie-agent-worker.js` | Module worker, the page-side fallback for the agent: runs WebLLM and streams a cited answer over evidence the widget hands it. |
| `eddie-wasm.js`, `eddie.wasm` | wasm-bindgen glue (`--target no-modules`, for the classic worker) + retriever (`src/wasm.rs`). |
| `eddie-wasm-esm.js` | wasm-bindgen glue for the same `eddie.wasm` as an ES module (`--target web`), imported by the service worker. |
| `eddie-transformers-sw.js` | transformers.js 4.2.0 (`dist/transformers.web.js`, Apache-2.0) with its onnxruntime-web imports pointed at the ORT "bundle" build, so it loads without `import()`; only the service worker uses it. |

The four JS entry points are built by concatenating `widget/src/lib/*.js`
(pure helpers, exposed as `EddieLib`) with their main file. The engines
themselves are `lib/search-engine.js` and `lib/agent-engine.js`; the entry
files only bind them to a host (`self.postMessage`, `importScripts`,
static or dynamic imports). There is no bundler; edit `widget/src/**` and
rerun `bash widget/build.sh` (or `bash widget/build.sh --js-only` to skip
the WASM build and only reassemble the bundles from an earlier
`widget/pkg/` and `widget/pkg-esm/`). The build downloads
`transformers.web.js` once into `widget/vendor/` (SHA-256 pinned in
`build.sh`).

## Tests

```bash
node --test widget/test/*.test.js
```

The tests cover the pure modules (URL and version handling, lane
selection, download sizes and consent copy, streaming downloads with retry
and SHA-256 verification, model id selection, think-stripping, plan parsing,
evidence merging, citation post-processing), both engines against fake
WASM/WebLLM (init and consent flow, cache check, lane fallback, index
reload, fatal traps, load sharing, streaming, abort), the transports
(request/reply, reconnect after a stopped service worker, registration
without `.ready`) and the decisions behind `data-persist` and `data-warm`.

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
        data-persist="auto"              <!-- auto | off: keep the engines in a service worker across navigations -->
        data-warm="auto"                 <!-- auto | off | always: initialise search before the modal opens -->
        defer></script>
```

`?v=` on the script `src` (or, failing that, on `data-index-url`) is also
appended to `eddie-worker.js`, `eddie-wasm.js`, `eddie.wasm`,
`eddie-agent-worker.js` and `eddie-sw.js` so a redeploy never pairs cached
glue with a new binary (the service worker is registered with
`updateViaCache: "none"`, so its static imports are revalidated too). When
`data-index-url` is absent the index is `index.ed` next to the widget
script.

## Host element

The widget mounts as `<div id="eddie-host">` (closed Shadow DOM) and mirrors
its state on that element for page CSS and tests:

| Attribute | Values |
|---|---|
| `data-theme` | `auto`, `light`, `dark` (from `data-theme` on the script tag) |
| `data-state` | `idle`, `loading`, `index_ready`, `awaiting_consent`, `ready`, `error`, `dead` |
| `data-lane` / `data-runtime` | dense lane id and `wasm` or `webgpu` once ready (empty when no lane loaded) |
| `data-arms` | comma-separated arms in use once ready, e.g. `dense,sparse,bm25` |
| `data-transport` / `data-agent-transport` | `sw` (service worker) or `worker` once the search / agent transport exists |
| `data-reused` / `data-agent-reused` | `true` when the ready engine / loaded model was taken over from the service worker instead of loaded |
| `data-ready-ms` / `data-agent-done-ms` | `performance.now()` when search became ready / the last answer finished (for measurements) |
| `data-persist` / `data-warm` | the parsed script-tag settings |

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
   `eddie.agent.consent`), loads WebLLM (in the service worker when it has
   WebGPU, else in a page-side module worker), plans 1–3 queries
   (JSON-schema constrained), runs them plus the raw question through the
   search engine (hybrid, top 6), merges up to 6 evidence chunks, and
   streams the answer with `[n]` citations. Typing a new query, Stop or Esc
   aborts generation. Data Saver disables the agent.

The steps above run in whichever host the transport picked; the next
section says how that is decided.

## Persistent engines

Dedicated workers die with the document, so every navigation used to
re-fetch the index, re-create the dense model session (about 3.5 s for
the Qwen3-Embedding ONNX lane on an RTX 4090) and reload the WebLLM
engine (5 to 16 s depending on the GPU shader cache). The widget now keeps both engines in one module
service worker, `eddie-sw.js`, registered with scope = the asset directory
(`/eddie/` by default). Pages outside that scope are not controlled by it
and never will be: the worker has no `fetch` handler, so the browser does
not start it for navigations, and pages talk to it through
`registration.active.postMessage`, never `navigator.serviceWorker.controller`
(nor `.ready`, which only resolves for the controlling registration).

**Transport choice** (`lib/transport.js`). After `load`, from an idle
callback, the widget registers the service worker (`data-persist="auto"`,
`navigator.serviceWorker` present, secure context) and opens a
`MessageChannel` to it; the worker must answer `hello` within 3 s. A modal
opened before that decision waits at most 3 s, then starts page-side
workers for this page. Anything that fails (no service worker support,
`register()` rejected because a CDN import failed, no `hello`) means
page-side workers, which speak exactly the same protocol; `data-persist="off"`
forces them. One exception: when the service worker reports no WebGPU
(`hello.onnx === false`) but the page has an adapter and
`data-dense-runtime` allows WebGPU, search stays page-side so the
webgpu-onnx lane is not silently replaced by the wasm lane; the agent still
uses the service worker if it can.

**Keepalive.** Chrome stops an idle service worker after about 30 s and
the transferred ports die with it. The widget pings every 15 s while the
modal is open, an answer is streaming or a request is pending. A ping
without `pong` within 5 s reconnects (new channel, new `hello`) and emits
`reset`: the widget re-runs init when the modal is open, and the next Ask
reloads the model. Any "index not loaded yet" / "model not loaded" reply
does the same transparently and retries once. With the modal closed the
worker is allowed to idle out; the next page then finds an empty engine
and warm-up (below) initialises it again.

**State reuse.** `hello` and `state` carry both engines' snapshots. A new
page whose index URL (including `?v=`) matches a `ready` engine mirrors
that state at once (`data-reused="true"`) instead of sending `init`. On
Ask, a loaded model with the chosen id is reused without a `load`.

**Warm at load** (`data-warm`, `lib/warm.js`). After `load` and an idle
callback, once the transport is decided: `auto` sends `cache_check` (which
fetches only `eddie.wasm` and the index, site assets the Hugo partial also
prefetches) and then `init` when the visitor accepted this lane before on
this browser (`localStorage` `eddie.search.consent`, written on accept and
on every `ready` with a lane) and the lane's files are in IndexedDB; it
never downloads a model, and does nothing under Data Saver. `always` also
downloads an uncached lane without asking (the site owner's choice;
still not under Data Saver). `off` waits for the first search. A service
worker that already reports `ready` is adopted without any of this.

**Redeploys.** `?v=` changes the service worker URL, so the browser
installs a new worker; an `init` with a different index URL reloads the
index inside a live worker. A trapped WASM panic leaves the engine dead
until the browser restarts the worker: the page falls back to page-side
workers for the rest of its life (Retry) and the next page gets a fresh
worker once Chrome has stopped the old one.

**Why static imports.** `import()` is disallowed in service workers. The
worker therefore statically imports the ES-module WASM glue
(`eddie-wasm-esm.js`, over the same `eddie.wasm`), WebLLM from
`cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm` (service worker script
fetches reject redirects, which rules out the `esm.run` alias the page
worker uses) and `eddie-transformers-sw.js`, a copy of transformers.js
whose `onnxruntime-web` imports point at the ORT bundle build. The stock
ORT build loads its WASM binding with `import()`; the bundle build embeds
it, provided `env.useWasmCache = false` and `wasmPaths` names only the
`.wasm` file (the service worker sets both). If any of those imports fail,
`register()` rejects and the widget uses page-side workers.

**Not covered.** Firefox and Safari have no WebGPU in service workers
today, so there the service worker hosts the WASM lane only and the agent
runs page-side (or search stays page-side when the page itself has
WebGPU, see above). Cross-origin asset hosting (widget script on a CDN)
cannot register a service worker for the page's origin; the widget falls
back to page workers.

## Worker protocol

The same messages go to `eddie-worker.js` (dedicated worker) or over a
`MessageChannel` port to `eddie-sw.js`; the service worker additions are
listed after the two worker tables.

Main thread → `eddie-worker.js`:

| Message | Fields |
|---|---|
| `init` | `indexUrl`, `baseUrl`, `version?`, `denseRuntime?`, `consent?`, `consentLane?` (re-sending `init` resumes after consent or retries after an error; a new `indexUrl` reloads the index) |
| `cache_check` | `requestId`, plus the `init` fields so it can load the index first |
| `search` | `requestId`, `query`, `topK?`, `mode?` (`hybrid`/`dense`/`sparse`/`keyword`), `evidence?` (attach best chunk text), `qa?` (k) |
| `page` / `chunk` / `qa` | `requestId` + `url` / `id` / `query`, `k?` |

Worker → main thread:

| Message | Fields |
|---|---|
| `status` | `state`: `loading_wasm`, `loading_index {progress}`, `index_ready {manifest, lanes}`, `consent_required {lane, sizeBytes, saveData}`, `downloading_model {file, progress, loaded, total}`, `loading_model {lane}`, `error {message, fatal, unsupported}` |
| `ready` | `lanes`, `lane`, `runtime` (`wasm`/`webgpu`), `arms {dense, sparse, bm25}`, `degraded[]`, `manifest`, `hostSkippedLanes[]` |
| `cache_result` | `requestId`, `cached`, `lane`, `sizeBytes`, `hostSkippedLanes[]`, `phase` |
| `search_result` | `requestId`, `results[]` (PageResult, plus `text` when `evidence`), `arms`, `degraded[]`, `mode`, `lane`, `qa?` |
| `page_result` / `chunk_result` / `qa_result` | `requestId` + `page` / `chunk` / `hits` |
| `error` | `requestId?`, `message`, `fatal`, `unsupported` |

Main thread → `eddie-agent-worker.js`: `load {model}`, `plan {requestId, question, site}`,
`ask {requestId, question, site, evidence: [{title, url, text}]}`, `abort {requestId?}`.
Back: `progress {text, progress}`, `loaded {model, loadMs}`, `plan_result {requestId, queries}`,
`token {requestId, text}`, `done {requestId, answer, citations: [{n, url, title}], nohit, usage}`,
`aborted {requestId}`, `error {requestId?, message}`.

Service worker (`eddie-sw.js`): the page posts `connect {kind: "search" | "agent", version}`
to `registration.active` with one `MessagePort` transferred; every later
message travels over that port, replies come back on it, and `status`,
`ready` and `progress` are fanned out to every connected port of the same
kind. On any port: `hello {requestId}` → `hello {requestId, ok, gpu, onnx, startedAt, search, agent}`,
`ping {requestId}` → `pong {requestId}`, `state {requestId}` → `state {requestId, gpu, onnx, search, agent}`
where `search` is `{phase, indexUrl, version, indexLoaded, lane, runtime, arms, degraded, manifest, lanes, hostSkippedLanes}`
and `agent` is `{model, loaded, loading, active}`; `disconnect` closes the
port and aborts an answer that page was receiving. `gpu` says whether the
worker has `navigator.gpu`; `onnx` whether it can run webgpu-onnx lanes.

## Content-Security-Policy

Nothing is bundled from a CDN; the libraries load on demand. A site with a
CSP needs:

```
script-src  'self' https://cdn.jsdelivr.net https://esm.run;
worker-src  'self' blob:;
connect-src 'self' https://huggingface.co https://*.hf.co https://cdn.jsdelivr.net https://esm.run https://raw.githubusercontent.com;
```

- `cdn.jsdelivr.net`: transformers.js (WebGPU dense lanes only), and for the
  service worker also WebLLM (`/npm/@mlc-ai/web-llm@…/+esm`) and the
  onnxruntime-web bundle module plus its `.wasm` (`connect-src`).
- `esm.run` (and its jsDelivr redirects): WebLLM in the page-side agent worker.
- `huggingface.co` / `*.hf.co`: model files (the resolve endpoint redirects to `cdn-lfs*.hf.co` / `*.aws.cdn.hf.co`).
- `raw.githubusercontent.com`: WebLLM's model-library WASM files, and `blob:` workers for WebLLM's internal use.
- The service worker script and its static imports are covered by
  `worker-src`/`script-src` (`'self'` for `eddie-sw.js`, `eddie-wasm-esm.js`
  and `eddie-transformers-sw.js`, `https://cdn.jsdelivr.net` for the CDN
  modules). No `Service-Worker-Allowed` header is needed: the scope stays
  under the asset directory.
- Without the agent and without WebGPU lanes, `script-src 'self'` and `connect-src 'self' https://huggingface.co https://*.hf.co` are enough (add `https://cdn.jsdelivr.net` to `script-src` when the service worker is on; it imports WebLLM and transformers.js at install time regardless).
