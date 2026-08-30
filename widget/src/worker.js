// SPDX-License-Identifier: GPL-3.0-only

// Eddie search worker (classic dedicated worker): the fallback host when the
// service worker (eddie-sw.js) is unavailable, and the same protocol either
// way. The engine itself is widget/src/lib/search-engine.js; this file only
// binds it to a dedicated worker: the WASM glue comes in through
// importScripts (no-modules builds: eddie-lite.js first, eddie-dense.js only
// when a CPU dense lane is about to run) and transformers.js through a
// dynamic import(). widget/build.sh concatenates widget/src/lib/*.js ahead
// of this file, so the helpers are available as `EddieLib`.
//
// Protocol: see widget/README.md ("Worker protocol").

"use strict";

const TRANSFORMERS_URL = "https://cdn.jsdelivr.net/npm/@huggingface/transformers@4.2.0";

const lib = EddieLib;

const loaded = {}; // variant -> Promise of the wasm-bindgen API object

function loadVariant(baseUrl, version, variant) {
  if (variant !== "lite" && variant !== "dense") throw new Error(`unknown wasm variant ${String(variant)}`);
  if (!loaded[variant]) {
    loaded[variant] = (async () => {
      importScripts(lib.assetUrl(baseUrl, `eddie-${variant}.js`, version));
      // eddie-dense.js is renamed by build.sh so both glues can share this
      // global scope (a second `let wasm_bindgen` would be a SyntaxError).
      const api = variant === "dense" ? wasm_bindgen_dense : wasm_bindgen;
      await api({ module_or_path: lib.assetUrl(baseUrl, `eddie-${variant}.wasm`, version) });
      return api;
    })();
    loaded[variant].catch(() => {
      delete loaded[variant]; // a failed fetch may be retried
    });
  }
  return loaded[variant];
}

const engine = lib.createSearchEngine({
  post: (message) => self.postMessage(message),
  loadWasm: (baseUrl, version, variant) => loadVariant(baseUrl, version, variant || "lite"),
  loadTransformers: () => import(TRANSFORMERS_URL),
  canRunWebGpuLane: true,
});

self.onmessage = function (e) {
  engine.handle(e.data || {}, (message) => self.postMessage(message));
};
