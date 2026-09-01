<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Efficient defaults, pass 2: tiers, caching and what is left

Date: 2026-09-01, on top of
[pass 1](2026-08-30-efficient-defaults.md) (boot loader, lite-first wasm,
service workers by tier, sidecars, bundled models) and the 0.4.2 Rust work
in [2026-08-30-wasm-size.md](2026-08-30-wasm-size.md).

Pass 1 fixed *what* a visitor downloads on the default path. Pass 2 fixes
the two places where they still downloaded something they had no use for:
bytes belonging to a tier they had not opted into, and bytes they already
had in cache.

## Changes

1. **The agent is its own service worker tier.** `sw/gpu` used to import
   WebLLM statically, so accepting the WebGPU *search* lane fetched 1.8 MB
   of agent runtime from jsDelivr whether or not the visitor ever asked a
   question. There are now four tiers: `sw/gpu` keeps lite search plus
   transformers.js and drops WebLLM (21.2 KB brotli to 16.4 KB), and the
   new `sw/agent` scope hosts `eddie-sw-agent.js` (8.7 KB) with WebLLM and
   the agent engine and no search at all, registered exactly at agent
   consent on the first Ask. It is never remembered as a search tier. The
   page-worker fallback (`eddie-agent-worker.js`) is unchanged.
2. **Each entry point bundles only the lib files it uses.** The widget was
   carrying the agent's prompt and tool code, and the worker was carrying
   lane copy it never rendered. Splitting `lib/agent.js` into
   `agent.js` + `agent-llm.js`, lifting the shared consent and status copy
   into `lib/copy.js`, and trimming `lib/lanes.js` took `eddie-widget.js`
   from 29,225 to 25,943 bytes brotli, below the 26,167 it weighed in
   0.4.1 before any of this work.
3. **Two `?v=` values instead of one.** Every runtime asset URL used to
   carry the *index's* `?v=`. On a static/ Hugo site that value is the
   build timestamp, so every content deploy gave `eddie-widget.js`,
   `eddie-lite.wasm`, the glue and the service worker new URLs: a returning
   visitor re-downloaded the engine, and the browser reinstalled the
   service worker, although none of those files had changed.
   `widget/build.sh` now hashes its own inputs (`widget/src/**`, the wasm
   binaries and their glue, the vendored transformers.js copy, the pinned
   versions, and itself) and stamps the first 12 hex into every bundle as
   `EDDIE_ASSET_VERSION`. The runtime uses it for the widget, the workers,
   the service workers, both wasm modules and their glue, and build.sh
   writes the same `?v=` into the service workers' static import
   specifiers. The index, its sidecars and site-bundled model files keep
   the index version: one `eddie index` run writes those together. An
   explicit `?v=` on the boot script's `src` still wins, so a site can pin
   a build; a bundle without the stamp falls back to the index version and
   behaves as before. The build fails if any entry point is missing the
   stamp. See requirement
   [0520](../../requirements/0400-widget-ui/0500-persistent-runtime/0520-asset-versioning.md).
4. **`--bundle-model` writes the exact byte count** into the manifest
   (`runtime.bytes`), so the consent card states the real download size
   instead of adding up HEAD requests or, worse, quoting the f32 originals'
   size for an f16 bundle.
5. **`_headers` matches the default path.** Only `eddie-boot.js` (the one
   URL a page names without a `?v=`) revalidates; the `eddie-widget.js`
   rule is commented out and documented as the `loader = "full"` trade.

## Asset list (dist/, brotli)

| File | Role | pass 1 | now |
|---|---|---:|---:|
| `eddie-boot.js` | default loader, every page view | 3,172 | 3,330 |
| `eddie-widget.js` | full widget, first interaction | 29,225 | 25,943 |
| `eddie-worker.js` | search engine, page-worker host | 14,494 | 15,056 |
| `eddie-agent-worker.js` | agent, page-worker host | 6,547 | 6,733 |
| `eddie-lite.wasm` | retriever without model code | 200,075 | 200,086 |
| `eddie-lite.js` / `-esm.js` | classic / module glue | 3,929 / 3,814 | 3,929 / 3,814 |
| `eddie-dense.wasm` | retriever + candle embedder | 732,643 | 731,823 |
| `eddie-dense.js` / `-esm.js` | classic / module glue | 4,212 / 4,081 | 4,212 / 4,081 |
| `eddie-sw-lite.js` | scope `sw/lite/` | ~16,000 | 16,616 |
| `eddie-sw-dense.js` | scope `sw/dense/` | ~15,900 | 16,612 |
| `eddie-sw-gpu.js` | scope `sw/gpu/`, no WebLLM | 21,187 | 16,622 |
| `eddie-sw-agent.js` | scope `sw/agent/`, WebLLM only | — | 8,724 |
| `eddie-transformers-sw.js` | transformers.js copy | 168,074 | 168,074 |

The boot loader, widget and worker each carry about 200 bytes more than
their pass-2 low point: that is the `EDDIE_ASSET_VERSION` constant and the
comments explaining the two versions. It buys the redeploy row below.

## Measurements

Microsoft Edge headless, driven by Playwright against a local static
server that brotli-compresses what Cloudflare Pages would (JS, wasm; `.ed`
and model files travel raw) and serves `hugo-module/example/_headers`'s
cache policy. Site bytes are counted at that server. External bytes are
counted at a logging proxy the browser is pointed at, not from the page's
own network events: a service worker's `fetch` never surfaces in the page
context, so a page-level listener would have reported "no external
traffic" whether or not the worker was downloading a model. The site is
www.jason-grey.com built
by Hugo 0.165 against this branch's `hugo-module`, with an index built by
this branch's CLI:

```
eddie index --content-dir .../content --cms hugo \
  --dense-model minilm   --dense-runtime wasm-candle \
  --dense-model bge-small --dense-runtime webgpu-onnx:Xenova/bge-small-en-v1.5:q8 \
  --sparse --qa --qa-heuristics --bundle-model minilm
```

435 chunks over 75 pages, 226 QA entries: `index.ed` 541,846,
`index.minilm.ed` 82,172 (QA vectors), `index.bge-small.ed` 192,476
(chunks + QA), `models/minilm/` 45.6 MB as bundled f16. WebGPU is present
(SwiftShader) except where a row says otherwise. The widget mounts a closed
shadow root, so the harness reopens it via `attachShadow` to click the real
consent buttons; nothing else about the page is instrumented.

Site numbers below are `/eddie/*` only. The rest of the page (HTML, CSS,
fonts, images) is 447 KB on a first visit and is unchanged by any of this;
so is the browser's own telemetry, which the proxy also sees and which is
excluded here.

The tiers' import graphs, which is what the whole split comes down to:

```
eddie-sw-lite.js   ./eddie-lite-esm.js
eddie-sw-dense.js  ./eddie-lite-esm.js  ./eddie-dense-esm.js
eddie-sw-gpu.js    ./eddie-lite-esm.js  ./eddie-transformers-sw.js
eddie-sw-agent.js  https://cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm
```

The gpu worker has no WebLLM import at all, so a WebGPU search consent
cannot fetch it; the agent worker has no wasm and no transformers.js.

| Scenario | Site bytes | External |
|---|---:|---:|
| (a) plain page view, first visit | **3,330** (`eddie-boot.js`, the whole cost) | 0 |
| (b) first open + one keyword/sparse search, no consent | **791,841** (index 541,846, lite wasm 200,086, widget 26,149, `eddie-sw-lite.js` 16,616, glue 3,814, boot 3,330) | 0 |
| (c) CPU dense lane accepted, browser without WebGPU | **47,216,790** (f16 weights 45,441,912, `eddie-dense.wasm` 731,823, tokenizer 148,046, `index.minilm.ed` 82,172, `eddie-sw-dense.js` 16,612, dense glue 4,081, config 303, on top of (b) minus the sidecar) | 0 |
| (d) WebGPU search lane accepted, no agent | **1,169,013** ((b) plus `index.bge-small.ed` 192,476, `eddie-transformers-sw.js` 168,074, `eddie-sw-gpu.js` 16,622) | **39,100,991** (ONNX q8 weights 34,084,532 from `us.aws.cdn.hf.co`, ORT asyncify wasm 4,793,017 from jsDelivr, 223,442 of config/tokenizer/api from huggingface.co) |
| (e) agent accepted, no search lane accepted | **800,583** ((b) plus `eddie-sw-agent.js` 8,742, and nothing else) | WebLLM 1,834,414 from jsDelivr, its runtime wasm 6,100,268 from raw.githubusercontent.com, ~400 MB of Qwen3 weights from `us.aws.cdn.hf.co` and 54,700 of config from huggingface.co |

Row (d) really ran the lane: `data-lane="bge-small"`,
`data-runtime="webgpu"`, `data-arms="dense,sparse,bm25"`, ready 23 s after
the click. What is not in it is WebLLM. In pass 1 the same click also pulled
1.82 MB of WebLLM plus a 38 KB ORT bundle from jsDelivr, because `sw/gpu`
imported them; the 4.79 MB of jsDelivr traffic above is the ORT asyncify
wasm transformers.js needs and nothing else.

Row (e) is the same split seen from the other side. The visitor never
accepted a search lane -- `data-tier` stayed `lite` -- pressed Ask, and
accepted the agent. The site served one extra file, `eddie-sw-agent.js`,
and the browser fetched WebLLM and its weights. No `eddie-sw-gpu.js`, no
168 KB transformers.js copy, no ORT: in pass 1 the agent lived in the gpu
tier, so accepting it dragged both in.

External bytes here are TLS-stream bytes at the proxy, so they are an upper
bound on payload and the weight figure is approximate; the origin set is
exact, and it is the origin set that the tier split is about.


### Caching: what a returning visitor pays

Same profile across all three rows: a visitor who has already opened the
search once (row (b)'s browser state), then comes back.

| Event | Site bytes |
|---|---:|
| content-only redeploy (new index `?v=`, `dist/` unchanged) | **541,846** — the new index, and nothing else: `eddie-boot.js` answers 304, the widget, the wasm, the glue and the service worker are all cache hits and the worker is not reinstalled |
| Eddie upgrade (new asset version, index unchanged) | **268,967** — the runtime, and *not* the index |
| both at once (an upgrade shipped with a content deploy) | 810,813 |

Before this change every redeploy was the third row: the index `?v=` drove
every URL, so a content deploy invalidated the engine too. The upgrade row
includes 18,973 bytes of page-worker path (`eddie-worker.js` 15,044 +
`eddie-lite.js` 3,929) that the service-worker path does not normally
fetch: while the new worker installs, the widget's 3-second transport
deadline expires and that page falls back to a page-side worker. It is a
one-page, once-per-upgrade cost and the fix (waiting longer) would make the
first search after an upgrade slower, so it stands.

### Progressive enhancement

Same site, same build, one capability removed at a time.

| Missing | Page view | First search |
|---|---|---|
| JavaScript | nothing from `/eddie/` at all; no trigger, no worker | the site is a plain static site, as it was before Eddie |
| service workers (registration blocked) | `eddie-boot.js`, trigger drawn | falls back to page-side workers -- `eddie-widget.js`, `eddie-worker.js`, `eddie-lite.js`, `eddie-lite.wasm`, `index.ed` -- and returns the same results. One log line says so |
| WebGPU | `eddie-boot.js`, trigger drawn | the `webgpu-onnx` lane is skipped and the CPU `wasm-candle` lane is offered instead (scenario (c)); with neither, keyword + sparse results and a "keyword-only" notice |
| nothing (Data Saver / `prefers-reduced-data`) | `eddie-boot.js`, trigger drawn, **nothing else** -- no warm-up, no idle widget fetch, no service worker | a search still works; only the automatic downloads are withheld |

The service-worker rung turned up one rough edge, now fixed: a container
that resolves `register()` with `undefined` rather than rejecting (some
privacy modes, and Playwright's `serviceWorkers: "block"`) produced
"Cannot read properties of undefined (reading 'active')" in the fallback
message. It falls back either way; it now says "registration was refused".

## Follow-ups

- The five pass-1 follow-ups are all closed: service worker imports now
  carry a `?v=`, the gpu tier no longer bundles the agent, the widget is
  back under its 0.4.1 size, the `serde_json` cut is measured and declined
  (see 2026-08-30-wasm-size.md), and the index `?v=` no longer busts the
  runtime.
- The index is now the dominant cost of a first search (541 KB of 792 KB on
  this site) and it is content, not overhead. The one part of it that is
  not site-specific is the 108 KB embedded WordPiece vocabulary; it exists
  to replace a 711 KB per-visitor fetch from huggingface.co, so it stays,
  but a site with several indexes pays for it in each.
- An upgraded service worker costs the first page 19 KB of page-worker
  path while it installs (see the caching table). Waiting longer instead
  would trade bytes for latency on the first search after an upgrade.
- `eddie-boot.js` is the only asset with no `?v=`, so it revalidates every
  300 s: a conditional request per page view for an unchanged 3.3 KB file.
  `stale-while-revalidate` would remove even that round trip on hosts that
  support it.
