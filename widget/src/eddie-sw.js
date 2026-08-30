// SPDX-License-Identifier: GPL-3.0-only

// Eddie service worker: a persistent host for the search engine and the
// agent, so a navigation within the site does not throw away the loaded
// index, the dense model or the WebLLM engine.
//
// One source, three builds (widget/build.sh), because a service worker may
// not import() anything: every dependency is a static import that build.sh
// prepends per tier, along with `EDDIE_SW_TIER`:
//
//   eddie-sw-lite.js   initLiteWasm / liteWasmApi (eddie-lite-esm.js)
//   eddie-sw-dense.js  lite + initDenseWasm / denseWasmApi (eddie-dense-esm.js)
//   eddie-sw-gpu.js    lite + `transformers` (eddie-transformers-sw.js, the
//                      copy whose onnxruntime-web imports point at the ORT
//                      bundle build) + `webllm` (jsDelivr; the esm.run alias
//                      redirects, and service worker script fetches reject
//                      redirects)
//
// Each tier is registered by the widget as a *module* service worker in
// its own scope under the asset directory (`/eddie/sw/<tier>/`), so a
// visitor who never accepts a model never installs the gpu tier's imports.
// The worker never handles `fetch`, so the browser does not start it for
// navigations; pages reach it through
// `registration.active.postMessage({type: "connect"}, [port])`, one
// MessageChannel per page and engine ("search" or "agent"), and then speak
// exactly the dedicated-worker protocols over that port. Three extra
// messages exist on every port: `hello` (answered with the host's tier and
// capabilities and both engines' state), `ping` -> `pong` (keepalive: Chrome
// stops an idle service worker after ~30 s) and `state`.
//
// widget/build.sh concatenates widget/src/lib/*.js ahead of this file.

"use strict";

const lib = EddieLib;

const SEARCH_TYPES = new Set(["init", "cache_check", "search", "page", "chunk", "qa"]);
const AGENT_TYPES = new Set(["load", "plan", "ask", "abort"]);
const startedAt = Date.now();
const hasGpu = !!(self.navigator && navigator.gpu && typeof navigator.gpu.requestAdapter === "function");
const canRunOnnx = EDDIE_SW_TIER === "gpu" && hasGpu;

/** A rejection that names the tier able to host what this one cannot; the widget moves the search there. */
function tierError(tier, what) {
  const e = new Error(`the ${EDDIE_SW_TIER} service worker has no ${what}; the ${tier} tier hosts it`);
  e.eddieTier = tier;
  return e;
}

const connections = new Set(); // { port, kind, reply }

function broadcast(kind, message) {
  for (const c of connections) {
    if (c.kind === kind) c.reply(message);
  }
}

/** ORT must use the factory embedded in its bundle build: no import() here. */
function configureTransformers(tf) {
  const onnx = tf.env && tf.env.backends && tf.env.backends.onnx;
  if (!onnx || !onnx.wasm) return;
  tf.env.useWasmCache = false;
  onnx.wasm.numThreads = 1;
  const ver = onnx.versions && onnx.versions.web;
  if (ver && !onnx.wasm.wasmPaths) {
    onnx.wasm.wasmPaths = { wasm: `https://cdn.jsdelivr.net/npm/onnxruntime-web@${ver}/dist/ort-wasm-simd-threaded.asyncify.wasm` };
  }
}

const wasmInits = {}; // variant -> Promise
function loadWasm(baseUrl, version, variant) {
  const v = variant || "lite";
  if (v === "lite") {
    if (!wasmInits.lite) wasmInits.lite = initLiteWasm({ module_or_path: lib.assetUrl(baseUrl, EDDIE_LITE_WASM, version) }).then(() => liteWasmApi);
    return wasmInits.lite;
  }
  if (v === "dense") {
    if (!initDenseWasm) return Promise.reject(tierError("dense", "CPU embedder"));
    if (!wasmInits.dense) wasmInits.dense = initDenseWasm({ module_or_path: lib.assetUrl(baseUrl, EDDIE_DENSE_WASM, version) }).then(() => denseWasmApi);
    return wasmInits.dense;
  }
  return Promise.reject(new Error(`unknown wasm variant ${String(variant)}`));
}

const searchEngine = lib.createSearchEngine({
  post: (message) => broadcast("search", message),
  loadWasm,
  loadTransformers: async () => {
    if (!transformers) throw tierError("gpu", "transformers.js");
    return transformers;
  },
  configureTransformers,
  // Lane *choice* follows the adapter, whatever the tier: a lite worker asks
  // consent for the webgpu lane, and the widget moves the search to the gpu
  // tier on accept (or on a tier_required status if the lane is cached).
  canRunWebGpuLane: hasGpu,
});

// Only the gpu tier bundles the agent (widget/src/lib/agent*.js) at all.
const agentEngine = webllm && typeof lib.createAgentEngine === "function"
  ? lib.createAgentEngine({
      post: (message) => broadcast("agent", message),
      loadWebLLM: async () => webllm,
    })
  : null;

function capabilities() {
  return {
    ok: true,
    tier: EDDIE_SW_TIER,
    gpu: EDDIE_SW_TIER === "gpu" && hasGpu,
    onnx: canRunOnnx,
    denseWasm: !!initDenseWasm,
    startedAt,
    search: searchEngine.state(),
    agent: agentEngine ? agentEngine.state() : null,
  };
}

function attach(port, kind) {
  const conn = {
    port,
    kind: kind === "agent" ? "agent" : "search",
    reply: (message) => {
      try {
        port.postMessage(message);
      } catch (err) {
        console.warn("eddie sw: reply failed", err);
      }
    },
  };
  connections.add(conn);
  port.onmessage = (e) => route(conn, e.data || {});
  // Chrome 132+ fires close when the page that owns the other end goes away.
  port.addEventListener("close", () => detach(conn));
  port.start();
}

function detach(conn) {
  if (!connections.has(conn)) return;
  connections.delete(conn);
  if (agentEngine) agentEngine.abortIfOwner(conn.reply);
  try {
    conn.port.close();
  } catch (_) {
    // ignore
  }
}

function route(conn, msg) {
  switch (msg.type) {
    case "hello":
      conn.reply(Object.assign({ type: "hello", requestId: msg.requestId }, capabilities()));
      return;
    case "ping":
      conn.reply({ type: "pong", requestId: msg.requestId });
      return;
    case "state":
      conn.reply(Object.assign({ type: "state", requestId: msg.requestId }, capabilities()));
      return;
    case "disconnect":
      detach(conn);
      return;
    default:
      break;
  }
  if (SEARCH_TYPES.has(msg.type)) {
    searchEngine.handle(msg, conn.reply);
  } else if (AGENT_TYPES.has(msg.type)) {
    if (agentEngine) agentEngine.handle(msg, conn.reply);
    else conn.reply({ type: "error", requestId: msg.requestId, message: `the ${EDDIE_SW_TIER} service worker has no agent; the gpu tier hosts it` });
  } else {
    conn.reply({ type: "error", requestId: msg.requestId, message: `unknown message type ${String(msg.type)}` });
  }
}

self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (e) => {
  e.waitUntil(self.clients.claim());
});

self.addEventListener("message", (e) => {
  const msg = e.data || {};
  if (msg.type === "connect" && e.ports && e.ports[0]) {
    attach(e.ports[0], msg.kind);
  }
});
