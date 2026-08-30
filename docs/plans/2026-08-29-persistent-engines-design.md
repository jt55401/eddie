<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Persistent engines (service worker host) and warm-at-load

Date: 2026-08-29. Branch `feat/persistent-engines`, on top of Eddie 0.4.1.

## Problem

The widget's search worker and agent worker are dedicated workers, so a
navigation within the site discards the loaded index, the dense model
session and the WebLLM engine. Measured on the reference machine (RTX 4090,
Chromium 151/Vulkan, no `shader-f16`, everything cached): WASM + bge-small
ready 0.4 s, Qwen3-Embedding ONNX session 3.7 s, WebLLM Qwen3.5-2B 14 to
16 s. Every page paid it again, and only after the visitor opened the modal.

## Design

Two hosts, one protocol. The engine logic moved out of the entry files into
`widget/src/lib/search-engine.js` and `widget/src/lib/agent-engine.js`;
each takes a `post` sink for broadcasts, a per-message `reply` sink, and
host hooks (`loadWasm`, `loadTransformers`, `loadWebLLM`). The entry files
`worker.js` and `eddie-agent-worker.js` bind them to dedicated workers and
stay protocol-compatible (they are the fallback). `widget/src/eddie-sw.js`
binds both to a module service worker whose module-level state outlives
pages.

Service worker facts that shaped it (all verified in Chromium 151):

- `import()` is disallowed in service workers (HTML spec, w3c/ServiceWorker#1356);
  everything is a static import. wasm-bindgen `--target web` emits the same
  binary as `--target no-modules` (build.sh compares the raw hashes), so one
  `eddie.wasm` serves both glues.
- Service worker script fetches reject redirects: `esm.run` fails at
  install; the direct `cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm`
  URL works.
- transformers.js's ORT backend loads its WASM binding with `import()` and
  fails in a service worker. The ORT "bundle" build embeds the binding and
  uses it only when `wasmPaths` has no `mjs` entry and transformers.js's
  `useWasmCache` blob trick is off. `build.sh` therefore ships
  `eddie-transformers-sw.js`: `transformers.web.js` (pinned, SHA-256) with
  its two `onnxruntime-web` imports rewritten to the bundle URL; the
  service worker sets `env.useWasmCache = false`, `numThreads = 1` and
  `wasmPaths = {wasm}`. Verified: Qwen3-Embedding-0.6B q4 on WebGPU runs
  inside the service worker.
- `navigator.gpu` is present in the service worker; `requestAdapter()` and
  `requestDevice()` work; WebLLM loads and generates there and survives a
  navigation (probe: second page answered with load 0 ms).
- `navigator.serviceWorker.ready` only resolves for the registration that
  controls the page; pages live outside the `/eddie/` scope, so the client
  waits on the registration's `statechange` instead.
- Chrome stops an idle service worker after ~30 s; transferred ports die
  with it. Keepalive pings every 15 s while the modal is open, an answer
  streams or a request is pending; a missed `pong` reconnects and resets.

Transport (`widget/src/lib/transport.js`): `DedicatedWorkerTransport` and
`ServiceWorkerTransport` with `call`/`on`/`postRaw`/`terminate`, chosen
after `load` from an idle callback; a modal open waits at most 3 s for the
decision. `data-persist="auto|off"`.

Warm at load (`widget/src/lib/warm.js`, `data-warm="auto|off|always"`):
`auto` runs `cache_check` (site assets only) and then `init` when the lane
was consented to before on this browser and is cached; `always` also
downloads; nothing under Data Saver. The Hugo partial adds
`<link rel="prefetch">` for `eddie.wasm` and the index. A service worker
that already reports `ready` for the same index URL is adopted.

## What changed for site owners

- Hugo params `persist` and `warm` (defaults `auto`).
- Three more runtime assets: `eddie-sw.js`, `eddie-wasm-esm.js`,
  `eddie-transformers-sw.js` (1.1 MB raw; only the service worker fetches it).
- CSP: `script-src`/`worker-src` must allow `https://cdn.jsdelivr.net` for
  the service worker's imports.

## Measurements

Reference machine (RTX 4090, Chromium 151/Vulkan, no `shader-f16`), local
static server, jason-grey.com index (qwen3e webgpu-onnx lane, Qwen3.5-2B
agent), Playwright harness driving the real widget. "Ready" is
`data-state=ready` measured from the page's time origin with the modal
closed (warm=auto, prior consent); "ask" is Shift+Enter to `done`.

| Scenario | Page 1 ready | Page 1 ask | Page 2 ready | Page 2 ask |
|---|---|---|---|---|
| persist=off, cached (baseline) | 3.66 s (page worker warm-up) | 7.5 s | 3.75 s | 7.7 s |
| persist=auto, cached | 3.54 s (service worker warm-up) | 7.3 s | 0.105 s (engine adopted) | 2.1 s (model reused) |
| persist=auto, cold profile | 40 s after consent (download) | 26.8 s (download) | 0.106 s | 3.2 s |

Token generation itself is unchanged (TTFT 0.5 to 0.9 s, 82 to 97 tok/s):
the page-2 gain is the skipped model load. A stopped service worker (CDP
`ServiceWorker.stopWorker`, equivalent to Chrome's idle stop) is detected
on the next modal open in 2 s and re-initialised transparently: results
2.2 s after opening. Stop shows "Stopped." within 10 ms and a following
Ask works (the generation lock is released).

Site owners who want the agent to survive longer reading pauses (Chrome
stops the worker ~30 s after the last message once the modal is closed)
could ping while the page is visible; that is a one-line policy change in
`keepaliveWanted` and was left at the specified behaviour.
