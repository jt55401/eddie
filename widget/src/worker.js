// SPDX-License-Identifier: GPL-3.0-only

// Eddie search worker (classic dedicated worker): the fallback host when the
// service worker (eddie-sw.js) is unavailable, and the same protocol either
// way. The engine itself is widget/src/lib/search-engine.js; this file only
// binds it to a dedicated worker: the WASM glue comes in through
// importScripts (no-modules build) and transformers.js through a dynamic
// import(). widget/build.sh concatenates widget/src/lib/*.js ahead of this
// file, so the helpers are available as `EddieLib`.
//
// Protocol: see widget/README.md ("Worker protocol").

"use strict";

const TRANSFORMERS_URL = "https://cdn.jsdelivr.net/npm/@huggingface/transformers@4.2.0";

const lib = EddieLib;

const engine = lib.createSearchEngine({
  post: (message) => self.postMessage(message),
  loadWasm: async (baseUrl, version) => {
    importScripts(lib.assetUrl(baseUrl, "eddie-wasm.js", version));
    await wasm_bindgen({ module_or_path: lib.assetUrl(baseUrl, "eddie.wasm", version) });
    return wasm_bindgen;
  },
  loadTransformers: () => import(TRANSFORMERS_URL),
  canRunWebGpuLane: true,
});

self.onmessage = function (e) {
  engine.handle(e.data || {}, (message) => self.postMessage(message));
};
