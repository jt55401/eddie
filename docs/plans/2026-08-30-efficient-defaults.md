<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Efficient defaults, pass 1: the JS and asset side

Date: 2026-08-30. Branch `feat/efficient-defaults`, on top of Eddie 0.4.2
(which brought the lite WASM variant, lane sidecars, bundled f16 models
and the embedded sparse vocabulary on the Rust side).

## Problem

Measured 2026-08-29 on www.jason-grey.com (0.4.1 widget, persistent
engines): every page view loaded the 26 KB widget and, at idle, installed a
service worker that statically imported the whole engine stack (WebLLM 1.8
MB and transformers.js 168 KB from jsDelivr, brotli); the first open
fetched a 733 KB `eddie.wasm` (3.5 MB raw, candle included), a 1.09 MB
index and a 712 KB `tokenizer.json` from huggingface.co before the first
keyword result. A visitor who never searched still paid about 2 MB; one who
searched once paid 4.6 MB; a visitor without WebGPU who accepted the dense
lane fetched 134 MB of f32 weights from huggingface.co.

## Decisions

1. **Boot loader by default.** `eddie-boot.js` (3.2 KB brotli) draws the
   trigger button and installs Ctrl/Cmd+K in its own closed Shadow DOM,
   reading the same `data-*` attributes; the full widget is fetched on the
   first pointerover/focusin/click of the trigger, on the shortcut, on
   `window.eddie.open()`, or after `load` (idle callback) when the visitor
   used the search or accepted a model before on this browser
   (`localStorage` `eddie.search.used` / `eddie.search.consent`) and
   neither Data Saver nor `prefers-reduced-data` is set. The hand-over is a
   `window.__eddieBoot` object the widget disposes when it mounts (opening
   the modal if a click or shortcut arrived while it loaded); a second
   mount is skipped. Hugo param `loader = "boot" | "full"`. The partial no
   longer emits `<link rel="prefetch">` for the wasm and index: those were
   speculative bytes for every visitor.
2. **Lite first, dense on demand.** The engine loads `eddie-lite.wasm`
   (200 KB brotli) and reads `capabilities()`. When a visitor accepts a
   `wasm-candle` lane it loads `eddie-dense.wasm` (733 KB) and hands the
   index over: `init_index` again, `attach_sidecar` for every sidecar it
   had attached, `init_sparse_tokenizer` if the vocabulary was fetched. The
   bytes for that hand-over are held only while a wasm-candle lane could
   still be chosen. The classic worker loads both glues with
   `importScripts`; build.sh renames the dense glue's global to
   `wasm_bindgen_dense` so two `let wasm_bindgen` never meet. Going
   lite-first even for returning consented visitors keeps one code path;
   the extra 200 KB is cached after the first visit.
3. **Service workers by tier.** One source, three builds with different
   static import blocks (a service worker cannot `import()`): lite
   (`eddie-lite-esm.js`), dense (+ `eddie-dense-esm.js`), gpu (+
   `eddie-transformers-sw.js` + WebLLM). Each registers in its own scope
   `<asset dir>sw/<tier>/`; the scripts stay flat in the asset directory so
   every installer copies one flat list. Registration is lazy: lite on the
   first open (or a returning visitor's warm-up), dense/gpu when a lane of
   that kind is accepted (`switchSearchTier`: new channel, `init` with
   consent, the old worker idles out), the agent always gpu. The tier is
   remembered in `localStorage` `eddie.search.tier`. Lane choice follows
   the WebGPU adapter in every tier; a host that cannot load the runtime
   its lane needs posts `tier_required` and the widget moves. The 0.4.2
   single-scope registration at `<asset dir>` is unregistered on the first
   transport setup. Page workers (`persist=off`, no service worker,
   registration failure) load everything themselves, as before.
4. **Index loading.** `manifest.sidecars`: the chosen lane's `chunks`
   sidecar is fetched (next to the index, with `?v=`) and attached before
   the model loads; the `qa` sidecar on the first QA lookup only (a lane's
   scopes share one file, so the webgpu lane's QA vectors cost nothing
   extra). `sparse_ready()` true after `init_index` skips the
   `tokenizer.json` download. `runtime.base_url` on a wasm-candle lane
   fetches the bundled files next to the index (cache name `@site/<file>`
   so the f16 copy never collides with a repo download). `eddie index
   --bundle-model` now writes the bundle's exact byte count into the
   manifest (`runtime.bytes`, optional and serde-defaulted); the consent
   card shows it directly, with one HEAD per file as the fallback for
   indexes that predate the field (the download-size table is never used
   for a bundle: it describes the f32 originals, twice the f16 copy). A webgpu-onnx lane with a
   `base_url` sets transformers.js `env.remoteHost` to that directory and
   `env.remotePathTemplate = "."` (the URL parser folds `./` away; the
   model id stays a valid HF id so transformers.js does not refuse it);
   the directory mirrors the repo's file layout. The consent copy names
   the model, the size, the origin ("this site" or "huggingface.co") and
   the sidecar bytes.
5. **Caching guidance and budgets.** `hugo-module/example/_headers` gives
   `/eddie/*` a one-year `immutable` Cache-Control (every asset URL carries
   `?v=<index hash>`) and keeps the two unversioned loader scripts
   revalidating. `scripts/report-asset-sizes.sh` fails the build on the
   default-path budgets below.
6. **Warm-up for returning visitors only.** `decideWarm` in `auto` returns
   `none` for a first-time visitor, so a plain page view with
   `loader = "full"` registers nothing either. The returning flag is read
   once at mount, before the first open on that page marks the visitor as
   returning (otherwise the modal's init and the warm-up's `cache_check`
   raced and fetched the index twice).
7. **`download.js` and compressed responses.** `readBody` trusted
   `Content-Length` as the decoded size and threw "response longer than
   Content-Length" for any `Content-Encoding: br` response. Production
   never hit it (Cloudflare does not compress `application/octet-stream`,
   so `.ed` travels raw, and its payload is brotli inside anyway), the
   measurement server did. Fixed: an encoded response is read to the end
   with indeterminate progress.

## Asset list (dist/)

| File | Role | brotli |
|---|---|---:|
| `eddie-boot.js` | default loader, every page view | 3.2 KB |
| `eddie-widget.js` | full widget, first interaction | 29.2 KB |
| `eddie-worker.js` | search engine, page-worker host | 14.5 KB |
| `eddie-agent-worker.js` | agent, page-worker host | 6.5 KB |
| `eddie-lite.wasm` | retriever without model code | 200 KB (709 KB raw) |
| `eddie-lite.js` | classic glue (global `wasm_bindgen`) | 3.9 KB |
| `eddie-lite-esm.js` | ES-module glue | 3.8 KB |
| `eddie-dense.wasm` | retriever + candle embedder | 733 KB (3.60 MB raw) |
| `eddie-dense.js` | classic glue (global `wasm_bindgen_dense`) | 4.2 KB |
| `eddie-dense-esm.js` | ES-module glue | 4.1 KB |
| `eddie-sw-lite.js` | service worker, lite tier, scope `sw/lite/` | 16.0 KB |
| `eddie-sw-dense.js` | service worker, dense tier, scope `sw/dense/` | 15.9 KB |
| `eddie-sw-gpu.js` | service worker, gpu tier, scope `sw/gpu/` | 21.2 KB |
| `eddie-transformers-sw.js` | transformers.js copy the gpu tier imports | 168 KB |

`eddie-lite-esm.wasm` / `eddie-dense-esm.wasm` appear only if wasm-bindgen
ever emits a different binary for `--target web` (build.sh compares the
hashes; today it does not). Gone: `eddie.wasm`, `eddie-wasm.js`,
`eddie-wasm-esm.js`, `eddie-sw.js`. `widget/pkg` (the npm package) is
still the dense no-modules build named `eddie`.

## Measurements

Reference machine (RTX 4090, Chromium 151/Vulkan for the headed runs,
headless for the no-WebGPU run), jason-grey.com content (75 pages), a
local static server that brotli-compresses what Cloudflare Pages would
(JS, wasm; `.ed` and model files travel raw), bytes counted at the server
for site assets and from response headers for huggingface.co / jsDelivr.
Playwright drives the real widget: Ctrl+K, "who has jason worked for",
Download, Shift+Enter, Download and answer.

Before: the 0.4.2 widget with the 0.4.1 index the site runs today (single
file, sparse vocabulary fetched, bge-small from huggingface.co). After:
this branch with an index built by the 0.4.2 CLI (`--preset gpu
--bundle-model bge-small --qa`): core 517 KB with the vocabulary embedded
and 141 QA entries, `index.qwen3e.ed` 534 KB, `index.bge-small.ed` 44 KB
(QA vectors), `models/bge-small/` 67 MB f16.

| Scenario | Before (0.4.2) | After |
|---|---:|---:|
| (a) plain page view, first visit, WebGPU browser | 2.07 MB (widget 26 KB, `eddie-sw.js` 18 KB, `eddie-wasm-esm.js` 4 KB, transformers 168 KB, WebLLM 1.82 MB + ORT 38 KB from jsDelivr) | **3.2 KB** (`eddie-boot.js`) |
| (a) plain page view, first visit, no WebGPU | 4.34 MB (the above plus, at idle, `eddie.wasm` 733 KB and the index 1.09 MB: warm auto ran `cache_check` for everyone) | **3.2 KB** |
| (b) first open + one keyword/sparse search, no consent | 2.53 MB (`eddie.wasm` 733 KB, index 1.09 MB, `tokenizer.json` 712 KB from huggingface.co) | **766 KB** (widget 29 KB, `eddie-sw-lite.js` 16 KB, `eddie-lite-esm.js` 4 KB, `eddie-lite.wasm` 200 KB, index 517 KB; nothing external) |
| (a)+(b) cumulative to the first result | 4.60 MB (6.87 MB without WebGPU) | **769 KB** |
| (c) after CPU dense consent (no WebGPU) | 134.2 MB from huggingface.co (f32 `model.safetensors` 133.5 MB, tokenizer 711 KB) | **67.7 MB** from the site (f16 weights 66.7 MB, tokenizer 154 KB br, `eddie-dense.wasm` 733 KB, `eddie-sw-dense.js` + glue 24 KB, `index.bge-small.ed` 44 KB on the first FAQ lookup) |
| (d) after WebGPU lane consent | 930.3 MB, all external (ONNX q4 repo 925.5 MB + 17 KB api from huggingface.co, ORT asyncify wasm 4.7 MB from jsDelivr) | **932.9 MB** (the same ONNX repo and ORT wasm, plus what 0.4.2 charged the page view and the open: WebLLM 1.82 MB + ORT bundle 38 KB from jsDelivr, `eddie-sw-gpu.js` 21 KB, transformers 168 KB, `index.qwen3e.ed` sidecar 534 KB from the site) |
| (d) + agent consent and one answer | 1.078 GB (Qwen3.5-2B weights 1.072 GB from hf-cdn, WebLLM runtime wasm 6.1 MB from raw.githubusercontent.com) | **1.078 GB** (identical: the weights dominate; WebLLM's JS was already fetched with the gpu tier) |

Same top results before and after in every run (the index content is the
same site). The heavy tiers cost what they cost (model weights dominate);
what changed is *when*: 0.4.2 fetched WebLLM and transformers.js on every
first page view of every visitor, this branch fetches them after the
visitor accepts the tier that needs them. The consent card now says what
will actually happen: "a one-time 67 MB download from this site" for the
bundled CPU lane, "a one-time 900 MB download from huggingface.co, plus
534 KB of index vectors from this site" for the WebGPU lane.

## Budgets (bytes, brotli unless stated; `scripts/report-asset-sizes.sh`, CI `widget-build`)

| Asset | Measured | Budget |
|---|---:|---:|
| `eddie-boot.js` | 3,172 | 4,096 |
| `eddie-widget.js` | 29,225 | 33,500 |
| `eddie-worker.js` | 14,494 | 16,500 |
| `eddie-lite.wasm` | 200,075 | 230,000 |
| `eddie-dense.wasm` raw / gzip / brotli | 3,597,135 / 1,067,755 / 732,643 | 3,700,000 / 1,150,000 / 820,000 |

## Follow-ups (pass 2 candidates)

All five were taken up; see
[2026-09-01-efficient-defaults-pass2.md](2026-09-01-efficient-defaults-pass2.md)
for what happened to each.

- The service worker import URLs (`./eddie-lite-esm.js`,
  `./eddie-transformers-sw.js`) carry no `?v=`; `updateViaCache: "none"`
  keeps them correct (fetched from the network at install) at the cost of
  a network fetch per install. A new `?v=` on the worker URL is a new
  install.
- The gpu tier's static imports mean a WebGPU-lane consent also fetches
  WebLLM (1.8 MB) even if the visitor never asks a question, and an agent
  consent fetches transformers.js. Splitting gpu into `gpu-search` and
  `agent` tiers would fix that at the cost of a fourth build.
- `eddie-widget.js` grew from 26.2 KB to 29.1 KB brotli with the tier and
  hand-over logic; the boot loader makes that a first-interaction cost
  rather than a page-view cost, but the widget itself has not been
  minified (no bundler by design).
- The lite module's remaining bulk is `serde_json` (101 KB of functions,
  see `2026-08-30-wasm-size.md`); binary `meta`/`qa` sections would be the
  next cut.
- Index `?v=` busts every asset on every site rebuild even when only the
  index changed; a per-asset content hash in the Hugo partial would keep
  the wasm cached across content deploys.
