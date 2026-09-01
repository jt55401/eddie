<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Eddie browser runtime

`widget/build.sh` produces fourteen files in `dist/`:

| File | Role |
|---|---|
| `eddie-boot.js` | Default loader (about 3 KB compressed, on every page view): draws the trigger button and installs Ctrl/Cmd+K, then fetches `eddie-widget.js` on the first interaction or, for a visitor who used the search before, after load. See [Loader](#loader). |
| `eddie-widget.js` | Search UI (closed Shadow DOM). Reads the `data-*` attributes on its `<script>` tag. |
| `eddie-worker.js` | Classic Web Worker, the page-side fallback for the search engine: loads the index, the WASM retriever and the dense model; answers `search`/`page`/`chunk`/`qa`. |
| `eddie-agent-worker.js` | Module worker, the page-side fallback for the agent: runs WebLLM and streams a cited answer over evidence the widget hands it. |
| `eddie-lite.wasm`, `eddie-lite.js`, `eddie-lite-esm.js` | The retriever without model code (`--no-default-features`: index parsing, BM25, learned sparse with the WordPiece query tokenizer built in, RRF, snippets, QA ranking, sidecars) and its wasm-bindgen glue for the classic worker (`--target no-modules`) and for the service workers (`--target web`). What every visitor who opens the search loads. |
| `eddie-dense.wasm`, `eddie-dense.js`, `eddie-dense-esm.js` | The same plus the candle BERT embedder for `wasm-candle` lanes. Fetched only after a visitor accepts a CPU dense lane; the engine hands the loaded index over to it. The classic glue's global is renamed `wasm_bindgen_dense` so both can live in one worker. |
| `eddie-sw-lite.js`, `eddie-sw-dense.js`, `eddie-sw-gpu.js`, `eddie-sw-agent.js` | Four builds of one module service worker source, each with the static imports of its tier (lite: `eddie-lite-esm.js`; dense: plus `eddie-dense-esm.js`; gpu: lite plus transformers.js; agent: WebLLM only), registered in their own scopes (see [Persistent engines](#persistent-engines)). |
| `eddie-transformers-sw.js` | transformers.js 4.2.0 (`dist/transformers.web.js`, Apache-2.0) with its onnxruntime-web imports pointed at the ORT "bundle" build, so it loads without `import()`; only the gpu service worker imports it. |

The JS entry points are built by concatenating `widget/src/lib/*.js`
(pure helpers, exposed as `EddieLib`) with their main file; the four
service worker files are the same `eddie-sw.js` source with a different
import block prepended. The engines
themselves are `lib/search-engine.js` and `lib/agent-engine.js`; the entry
files only bind them to a host (`self.postMessage`, `importScripts`,
static or dynamic imports). There is no bundler; edit `widget/src/**` and
rerun `bash widget/build.sh` (or `bash widget/build.sh --js-only` to skip
the WASM builds and only reassemble the bundles from an earlier
`widget/pkg*/`; `--sizes` prints raw, gzip and brotli bytes per file). The
build downloads `transformers.web.js` once into `widget/vendor/` (SHA-256
pinned in `build.sh`). `scripts/report-asset-sizes.sh` enforces the size
budgets CI runs (boot, widget, worker and lite wasm after brotli, plus the
dense module).

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
without `.ready`, tier selection, the 0.4.2 registration cleanup), the
decisions behind `data-persist`, `data-warm` and the boot loader, and the
lite-first flow (embedded sparse vocabulary, sidecar selection, the
lite-to-dense hand-over, site-bundled models).

## Loader

The Hugo partial (and every CMS integration) puts `eddie-boot.js` on the
page by default. It reads the same `data-*` attributes as the widget,
renders the trigger button in its own closed Shadow DOM (same position,
offsets and theme) and listens for the trigger, Ctrl/Cmd+K and
`window.eddie.open()`. The first of those injects
`<script src="eddie-widget.js?v=…">` with the attributes copied over; the
full widget removes the boot trigger when it mounts and opens the modal if
that is what the visitor asked for. Hovering or focusing the trigger only
preloads. `lib/boot.js` also loads the widget after `load` (idle callback)
when the visitor opened the search or accepted a model before on this
browser (`localStorage` `eddie.search.used` / `eddie.search.consent`) and
neither Data Saver nor `prefers-reduced-data` is on, or when
`data-warm="always"`; the widget then runs its own warm-up. A first-time
visitor's page view costs the boot script and nothing else. Sites that
want the full widget on every page (`loader = "full"` in Hugo) use the
script tag below with `eddie-widget.js` directly; loading both is
harmless (the second mount is skipped).

## Script tag

```html
<script src="/eddie/eddie-boot.js?v=<build hash>"
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
        data-dense-runtime="auto"        <!-- auto | wasm | webgpu | off: off is keyword + sparse, no model -->
        data-consent-text=""             <!-- override of the download consent copy; {size}, {model} and {origin} are substituted -->
        data-persist="auto"              <!-- auto | off: keep the engines in a service worker across navigations -->
        data-warm="auto"                 <!-- auto | off | always: initialise search before the modal opens (auto: returning visitors only) -->
        defer></script>
```

There are two `?v=` values, and they move on different schedules.

The **asset version** is a hash of the sources and binaries that produced
`dist/`, stamped into every bundle by `widget/build.sh` as
`EDDIE_ASSET_VERSION` and used for `eddie-widget.js`, `eddie-worker.js`,
`eddie-agent-worker.js`, `eddie-sw-*.js`, both wasm modules and their glue
(and for the service workers' own static imports, which build.sh writes
with the same value). It changes when Eddie is upgraded. An explicit `?v=`
on the boot script's `src` overrides it for `eddie-widget.js`, which is how
a site pins a build.

The **index version** is the `?v=` on `data-index-url`, which the Hugo
partial derives from the index's content (or the build time). It reaches
the index, its `index.<lane>.ed` sidecars and any site-bundled model files:
what one `eddie index` run writes together.

Sharing one value between them re-downloaded the whole engine, and
reinstalled the service worker, on every content deploy. Keeping them apart
means a rebuilt site fetches a new index and nothing else.

Every asset URL is versioned, so `/eddie/*` can be served with a one-year
`immutable` Cache-Control; `hugo-module/example/_headers` is a Cloudflare
Pages / Netlify example that does that while keeping `eddie-boot.js` (the
one URL the page names without a `?v=`) revalidating. When `data-index-url`
is absent the index is `index.ed` next to the widget script.

## Settings (the gear)

The modal's gear opens a panel where a visitor chooses what to download and
which models to run. Four preferences, stored together in `localStorage`
under `eddie.settings`:

| Group | Values | Effect |
|---|---|---|
| Search model | `none`, or a lane id from the index | `none` runs keyword + sparse and downloads no model; a lane id pins that lane |
| Answers | `off`, `light`, `quality` (or the site's pinned model id) | `off` hides the Ask button; the two levels are the Qwen3.5 0.8B and 2B builds |
| Preload | the `data-warm` values up to the site's | when search initialises relative to the modal opening |
| Between pages | the `data-persist` values up to the site's | whether the engines live in a service worker |

Below them, the quota-managed storage this origin holds (the model cache,
WebLLM's weights, whatever the service worker cached) and a button that
deletes it. While a search transport exists the engine owns the database, so
the deletion goes through it (`cache_clear`); otherwise the widget deletes
the database itself. Either way WebLLM's caches go too.

**The site config is the ceiling.** The panel offers a lane only if the index
carries it, this browser can run it, and `data-dense-runtime` allows it; the
agent only if `data-agent-mode` is not `off` and a WebGPU adapter exists;
`data-warm="off"` leaves `off` as the only preload option. A visitor can
always choose less than the site asks for, and can choose among what the site
left open, but cannot switch on what the owner turned off. A stored
preference that is no longer on offer -- a re-indexed site, a different
browser, a changed attribute -- falls back to the site default rather than
breaking the widget (`widget/src/lib/settings.js`).

Changing the search model re-initialises the engine, moving it to the tier
the new lane needs, and is remembered for the next page: a service worker
still running the previous lane is not adopted.

## What is fetched when

Measured 2026-08-30 on the jason-grey.com index (75 pages, 435 chunks,
bge-small `wasm-candle` lane bundled next to the index, Qwen3-Embedding
`webgpu-onnx` lane, learned sparse with the vocabulary embedded, 141 QA
entries), brotli where the host compresses:

| Step | Fetched | Bytes |
|---|---|---:|
| Page view, first visit | `eddie-boot.js` | 3.2 KB |
| First open, keyword + sparse search | `eddie-widget.js` 29 KB, `eddie-sw-lite.js` 16 KB + `eddie-lite-esm.js` 4 KB (or `eddie-worker.js` 14 KB + `eddie-lite.js` 4 KB page-side), `eddie-lite.wasm` 200 KB, `index.ed` 517 KB | 766 KB |
| CPU dense lane accepted (no WebGPU) | `eddie-sw-dense.js` + `eddie-dense-esm.js`, `eddie-dense.wasm` 733 KB, `models/bge-small/*` 67 MB (f16, from the site) | 68 MB |
| WebGPU lane accepted | `eddie-sw-gpu.js` + `eddie-transformers-sw.js` 168 KB + ORT bundle 38 KB (jsDelivr), `index.qwen3e.ed` 534 KB, the ONNX repo from huggingface.co | see the plan doc |
| Agent accepted (first Ask) | `eddie-sw-agent.js` + WebLLM 1.8 MB (jsDelivr), the model weights from HuggingFace | see the plan doc |
| First FAQ card / agent evidence | the active lane's QA sidecar (`index.<lane>.ed`, 44 KB for bge-small) | |

The 0.4.2 defaults for the same site were 2.07 MB on every first page
view (widget + service worker with WebLLM and transformers.js imported at
install) and 2.53 MB more on the first open (dense wasm, 1.09 MB index,
712 KB `tokenizer.json` from huggingface.co).

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
engine (5 to 16 s depending on the GPU shader cache). The widget keeps
both engines in module service workers instead. There are four builds
of one source, one per tier, because a service worker cannot `import()` and must
carry its dependencies as static imports:

| Tier | Script | Scope | Imports | Registered |
|---|---|---|---|---|
| lite | `eddie-sw-lite.js` | `/eddie/sw/lite/` | `eddie-lite-esm.js` | on the first modal open (or a returning visitor's warm-up) |
| dense | `eddie-sw-dense.js` | `/eddie/sw/dense/` | lite + `eddie-dense-esm.js` | when a `wasm-candle` lane is accepted |
| gpu | `eddie-sw-gpu.js` | `/eddie/sw/gpu/` | lite + `eddie-transformers-sw.js` | when a `webgpu-onnx` lane is accepted |
| agent | `eddie-sw-agent.js` | `/eddie/sw/agent/` | WebLLM | at agent consent, on the first Ask |

Pages outside those scopes are not controlled by the workers and never
will be: they have no `fetch` handler, so the browser does not start them
for navigations, and pages talk to them through
`registration.active.postMessage`, never `navigator.serviceWorker.controller`
(nor `.ready`, which only resolves for the controlling registration). A
plain page view by a first-time visitor registers nothing. The first
transport setup also unregisters the single-scope worker Eddie 0.4.2
registered at `/eddie/` (its script no longer exists).

**Transport choice** (`lib/transport.js`). When something needs an
engine, the widget registers that tier's worker (`data-persist="auto"`,
`navigator.serviceWorker` present, secure context) and opens a
`MessageChannel` to it; the worker must answer `hello` within 3 s and
report the expected tier. A modal opened before that decision waits at
most 3 s, then starts page-side workers for this page. Anything that fails
(no service worker support, `register()` rejected because a CDN import
failed, no `hello`) means page-side workers, which speak exactly the same
protocol and load the dense module or transformers.js themselves;
`data-persist="off"` forces them. Accepting a lane the current tier cannot
host moves the search to the right tier (`switchSearchTier`): a new
channel, `init` with consent, the old worker left to idle out. The tier is
remembered (`localStorage` `eddie.search.tier`) so the next page registers
it directly. The agent always uses the agent tier, registered at agent
consent on the first Ask, so a visitor who only accepts the WebGPU search
lane never fetches WebLLM. One exception: when the gpu worker reports no
WebGPU (`hello.onnx === false`) but the page has an adapter and
`data-dense-runtime` allows WebGPU, search stays page-side so the
webgpu-onnx lane is not silently replaced by the wasm lane; the agent
tier is unaffected.

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
callback: `auto` does nothing for a first-time visitor; for one who opened
the search before on this browser (`localStorage` `eddie.search.used`) it
registers the remembered tier and sends `cache_check` (which fetches only
`eddie-lite.wasm` and the index), then `init` when the visitor accepted
this lane before (`eddie.search.consent`, written on accept and on every
`ready` with a lane) and the lane's files are in IndexedDB; it never
downloads a model, and does nothing under Data Saver. `always` also
downloads an uncached lane without asking (the site owner's choice; still
not under Data Saver). `off` waits for the first search. A service worker
that already reports `ready` is adopted without any of this.

**Redeploys.** A new asset version changes the service worker URL, so the
browser installs a new worker; a content-only deploy leaves that URL alone
and the running worker just reloads the index (an `init` with a different
index URL). A trapped WASM panic leaves the engine dead
until the browser restarts the worker: the page falls back to page-side
workers for the rest of its life (Retry) and the next page gets a fresh
worker once Chrome has stopped the old one.

**Why static imports.** `import()` is disallowed in service workers. The
workers therefore statically import the ES-module WASM glue
(`eddie-lite-esm.js`, `eddie-dense-esm.js`), WebLLM from
`cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm` (service worker script
fetches reject redirects, which rules out the `esm.run` alias the page
worker uses) and `eddie-transformers-sw.js`, a copy of transformers.js
whose `onnxruntime-web` imports point at the ORT bundle build. The stock
ORT build loads its WASM binding with `import()`; the bundle build embeds
it, provided `env.useWasmCache = false` and `wasmPaths` names only the
`.wasm` file (the service worker sets both). If any of those imports fail,
`register()` rejects and the widget uses page-side workers. Only the gpu
tier pays for them, and only after a visitor opted in.

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

- `cdn.jsdelivr.net`: transformers.js (WebGPU dense lanes only), WebLLM
  (`/npm/@mlc-ai/web-llm@…/+esm`, the agent tier's import) and the
  onnxruntime-web bundle module plus its `.wasm` (`connect-src`).
- `esm.run` (and its jsDelivr redirects): WebLLM in the page-side agent worker.
- `huggingface.co` / `*.hf.co`: model files (the resolve endpoint redirects to `cdn-lfs*.hf.co` / `*.aws.cdn.hf.co`).
- `raw.githubusercontent.com`: WebLLM's model-library WASM files, and `blob:` workers for WebLLM's internal use.
- The service worker scripts and their static imports are covered by
  `worker-src`/`script-src` (`'self'` for `eddie-sw-*.js`,
  `eddie-*-esm.js` and `eddie-transformers-sw.js`,
  `https://cdn.jsdelivr.net` for the CDN modules the gpu and agent tiers
  import). No
  `Service-Worker-Allowed` header is needed: the scopes stay under the
  asset directory.
- Without the agent and without WebGPU lanes, `script-src 'self'` and `connect-src 'self' https://huggingface.co https://*.hf.co` are enough; an index whose models are bundled next to it (`eddie index --bundle-model`) and whose sparse vocabulary is embedded (the default) needs no `huggingface.co` at all.
