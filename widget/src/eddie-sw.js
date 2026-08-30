// SPDX-License-Identifier: GPL-3.0-only

// Eddie service worker: a persistent host for the search engine and the
// agent, so a navigation within the site does not throw away the loaded
// index, the dense model or the WebLLM engine.
//
// Registered by the widget as a *module* service worker scoped to the asset
// directory (`/eddie/` by default). It never handles `fetch`, so the browser
// does not start it for navigations; pages reach it through
// `registration.active.postMessage({type: "connect"}, [port])`, one
// MessageChannel per page and engine ("search" or "agent"), and then speak
// exactly the dedicated-worker protocols over that port. Three extra
// messages exist on every port: `hello` (answered with the host's
// capabilities and both engines' state), `ping` -> `pong` (keepalive: Chrome
// stops an idle service worker after ~30 s) and `state`.
//
// Dynamic import() is disallowed in service workers, so everything is a
// static import: the wasm-bindgen `--target web` glue (dist/eddie-wasm-esm.js,
// same eddie.wasm as the classic worker), WebLLM straight from jsDelivr (the
// esm.run alias redirects, and service worker script fetches reject
// redirects) and a copy of transformers.js whose onnxruntime-web imports
// point at the "bundle" build (dist/eddie-transformers-sw.js, produced by
// widget/build.sh): the stock build loads its WASM binding with import().
//
// widget/build.sh concatenates widget/src/lib/*.js ahead of this file.

"use strict";

import initWasm, * as wasmApi from "./eddie-wasm-esm.js";
import * as webllm from "https://cdn.jsdelivr.net/npm/@mlc-ai/web-llm@0.2.84/+esm";
import * as transformers from "./eddie-transformers-sw.js";

const lib = EddieLib;

const SEARCH_TYPES = new Set(["init", "cache_check", "search", "page", "chunk", "qa"]);
const AGENT_TYPES = new Set(["load", "plan", "ask", "abort"]);
const startedAt = Date.now();
const hasGpu = !!(self.navigator && navigator.gpu && typeof navigator.gpu.requestAdapter === "function");

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

let wasmInit = null;
const searchEngine = lib.createSearchEngine({
  post: (message) => broadcast("search", message),
  loadWasm: async (baseUrl, version) => {
    if (!wasmInit) wasmInit = initWasm({ module_or_path: lib.assetUrl(baseUrl, EDDIE_ESM_WASM, version) });
    await wasmInit;
    return wasmApi;
  },
  loadTransformers: async () => transformers,
  configureTransformers,
  canRunWebGpuLane: hasGpu,
});

const agentEngine = lib.createAgentEngine({
  post: (message) => broadcast("agent", message),
  loadWebLLM: async () => webllm,
});

function capabilities() {
  return {
    ok: true,
    gpu: hasGpu,
    onnx: hasGpu,
    startedAt,
    search: searchEngine.state(),
    agent: agentEngine.state(),
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
  agentEngine.abortIfOwner(conn.reply);
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
    agentEngine.handle(msg, conn.reply);
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
