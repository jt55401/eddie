// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");

// The engine reads its helpers from a lexical EddieLib (the bundles) or env.lib.
const lib = Object.assign({}, require("../src/lib/urls.js"), require("../src/lib/lanes.js"), require("../src/lib/download.js"));
const SE = require("../src/lib/search-engine.js");

const MANIFEST = {
  format: 5,
  eddie: "0.4.1",
  chunks: 3,
  pages: 2,
  sections: ["qa"],
  dense: [
    { id: "bge", model: "BAAI/bge-small-en-v1.5", family: "bert", dim: 4, revision: "abc", runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "model.safetensors"] } },
    { id: "qwen3e", model: "Qwen/Qwen3-Embedding-0.6B", family: "qwen3", dim: 4, runtime: { kind: "webgpu-onnx", repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX", dtype: "q4" } },
  ],
};

/** Fake wasm-bindgen API: records calls, serves canned results. */
function fakeWasm() {
  const calls = [];
  return {
    calls,
    manifest: () => JSON.stringify(MANIFEST),
    init_index: (bytes) => calls.push(["init_index", bytes.length]),
    init_sparse_tokenizer: () => calls.push(["sparse"]),
    init_dense_wasm: (id) => calls.push(["dense", id]),
    search: (q, k, mode, laneId, vec) => {
      calls.push(["search", q, k, mode, laneId, vec ? vec.length : null]);
      return JSON.stringify({ results: [{ url: "/a/", title: "A", chunk: 1, snippet: "s" }], arms: { bm25: true, dense: !!laneId }, degraded: [], mode, dense_lane: laneId });
    },
    chunk: (id) => JSON.stringify({ id, text: "chunk text " + id }),
    page: (url) => JSON.stringify({ url, chunks: [] }),
    qa_lookup: (q, laneId, vec, k) => JSON.stringify([{ question: q, answer: "a", score: 0.9, confident: true, k }]),
  };
}

function fakeFetch(map) {
  return async (url) => {
    if (!(url in map)) return { ok: false, status: 404, headers: { get: () => null } };
    const body = map[url];
    return { ok: true, status: 200, headers: { get: (h) => (h === "Content-Length" ? String(body.length) : null) }, body: null, arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.length) };
  };
}

function makeEngine(opts) {
  const o = opts || {};
  const posted = [];
  const wasm = o.wasm || fakeWasm();
  const engine = SE.createSearchEngine({
    lib,
    post: (m) => posted.push(m),
    loadWasm: async () => wasm,
    loadTransformers: o.loadTransformers || (async () => { throw new Error("no transformers in tests"); }),
    canRunWebGpuLane: o.canRunWebGpuLane,
    navigator: o.navigator === undefined ? {} : o.navigator,
    indexedDB: null,
    fetch: fakeFetch(Object.assign({ "https://site/eddie/index.ed": new Uint8Array([1, 2, 3]) }, o.files || {})),
  });
  return { engine, posted, wasm };
}

const INIT = { type: "init", indexUrl: "https://site/eddie/index.ed", baseUrl: "https://site/eddie/", version: "1", denseRuntime: "auto" };

test("init: loads wasm and index, reports statuses, then asks consent for the first dense lane", async () => {
  const { engine, posted, wasm } = makeEngine();
  await engine.handle(INIT);
  const states = Array.from(new Set(posted.filter((m) => m.type === "status").map((m) => m.state)));
  assert.deepEqual(states.slice(0, 3), ["loading_wasm", "loading_index", "index_ready"]);
  assert.deepEqual(wasm.calls[0], ["init_index", 3]);
  const consent = posted.find((m) => m.state === "consent_required");
  assert.ok(consent, "no IndexedDB means nothing is cached: consent is required");
  assert.equal(consent.lane.id, "bge", "no WebGPU adapter in this env: the wasm lane comes first");
  assert.equal(consent.sizeBytes, 134e6);
  assert.equal(engine.phase, "awaiting_consent");
  const st = engine.state();
  assert.equal(st.indexLoaded, true);
  assert.equal(st.indexUrl, INIT.indexUrl);
  assert.equal(st.manifest.chunks, 3);
  assert.deepEqual(st.lanes.map((l) => l.id), ["bge", "qwen3e"]);
});

test("keyword search works from index_ready on; requests before init get the not-loaded error", async () => {
  const { engine, posted } = makeEngine();
  const replies = [];
  await engine.handle({ type: "search", requestId: 1, query: "x" }, (m) => replies.push(m));
  assert.equal(replies[0].type, "error");
  assert.equal(replies[0].message, SE.NOT_LOADED);
  assert.equal(SE.isNotLoadedMessage(replies[0].message), true);
  assert.equal(replies[0].fatal, false);
  await engine.handle(INIT);
  await engine.handle({ type: "search", requestId: 2, query: "hello", topK: 3, qa: 2 }, (m) => replies.push(m));
  const r = replies[1];
  assert.equal(r.type, "search_result");
  assert.equal(r.requestId, 2);
  assert.equal(r.results[0].url, "/a/");
  assert.equal(r.lane, null);
  assert.equal(r.qa[0].k, 2, "qa hits ride along when the index has a qa section");
  assert.equal(posted.some((m) => m.requestId === 2), false, "replies go to the request sink, not the broadcast sink");
});

test("evidence search attaches chunk text; page/chunk/qa lookups answer on the reply sink", async () => {
  const { engine } = makeEngine();
  await engine.handle(INIT);
  const out = [];
  const sink = (m) => out.push(m);
  await engine.handle({ type: "search", requestId: 3, query: "q", evidence: true }, sink);
  assert.equal(out[0].results[0].text, "chunk text 1");
  await engine.handle({ type: "page", requestId: 4, url: "/a/" }, sink);
  assert.equal(out[1].page.url, "/a/");
  await engine.handle({ type: "chunk", requestId: 5, id: 9 }, sink);
  assert.equal(out[2].chunk.id, 9);
  await engine.handle({ type: "qa", requestId: 6, query: "why", k: 1 }, sink);
  assert.equal(out[3].hits[0].question, "why");
  await engine.handle({ type: "nope", requestId: 7 }, sink);
  assert.match(out[4].message, /unknown message type nope/);
});

test("consent: init with consent=true loads the lane files and reports ready with the arms", async () => {
  const files = {};
  const repo = "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/abc/";
  for (const f of ["config.json", "tokenizer.json", "model.safetensors"]) files[repo + f] = new Uint8Array([7]);
  const { engine, posted, wasm } = makeEngine({ files });
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const ready = posted.find((m) => m.type === "ready");
  assert.ok(ready, "ready after consent");
  assert.equal(ready.lane, "bge");
  assert.equal(ready.runtime, "wasm");
  assert.deepEqual(ready.arms, { dense: true, sparse: false, bm25: true });
  assert.ok(wasm.calls.some((c) => c[0] === "dense" && c[1] === "bge"));
  assert.equal(engine.state().phase, "ready");
  // A second init on a ready engine (another page connecting) re-posts ready.
  const n = posted.filter((m) => m.type === "ready").length;
  await engine.handle(INIT);
  assert.equal(posted.filter((m) => m.type === "ready").length, n + 1);
});

test("a lane that fails is degraded and the next candidate is tried; none left means keyword-only ready", async () => {
  const { engine, posted } = makeEngine({ files: {} }); // downloads 404
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const ready = posted.find((m) => m.type === "ready");
  assert.ok(ready);
  assert.equal(ready.lane, null);
  assert.equal(ready.arms.dense, false);
  assert.ok(ready.degraded.some((d) => /lane bge failed/.test(d)));
});

test("cache_check loads wasm+index and reports the candidate lane and its cache state", async () => {
  const { engine, posted } = makeEngine();
  const out = [];
  await engine.handle(Object.assign({ type: "cache_check", requestId: 11 }, INIT, { type: "cache_check" }), (m) => out.push(m));
  assert.equal(out[0].type, "cache_result");
  assert.equal(out[0].cached, false);
  assert.equal(out[0].lane.id, "bge");
  assert.equal(out[0].phase, "idle", "cache_check does not start the init state machine");
  assert.ok(posted.some((m) => m.state === "index_ready"), "index_ready is broadcast so the widget can search");
  assert.equal(engine.state().indexLoaded, true);
});

test("a host that cannot run webgpu-onnx lanes skips them even with a WebGPU adapter", async () => {
  const navigator = { gpu: { requestAdapter: async () => ({ features: new Set(), limits: { maxBufferSize: 1 } }) } };
  const withGpu = makeEngine({ navigator, canRunWebGpuLane: true });
  await withGpu.engine.handle(INIT);
  assert.equal(withGpu.posted.find((m) => m.state === "consent_required").lane.id, "qwen3e");
  assert.deepEqual(withGpu.engine.state().hostSkippedLanes, []);
  const noOnnx = makeEngine({ navigator, canRunWebGpuLane: false });
  await noOnnx.engine.handle(INIT);
  assert.equal(noOnnx.posted.find((m) => m.state === "consent_required").lane.id, "bge");
  assert.deepEqual(noOnnx.engine.state().hostSkippedLanes, ["qwen3e"]);
});

test("a new index URL (redeploy) reloads the index in a live engine", async () => {
  const { engine, wasm } = makeEngine({ files: { "https://site/eddie/index.ed?v=2": new Uint8Array([1, 2, 3, 4]) } });
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { indexUrl: "https://site/eddie/index.ed?v=2" }));
  assert.deepEqual(wasm.calls.filter((c) => c[0] === "init_index"), [["init_index", 3], ["init_index", 4]]);
  assert.equal(engine.state().indexUrl, "https://site/eddie/index.ed?v=2");
});

test("a WASM trap is fatal: the engine goes dead and refuses further work", async () => {
  const wasm = fakeWasm();
  wasm.search = () => {
    const e = new Error("unreachable executed");
    e.eddieFatal = true;
    throw e;
  };
  const { engine } = makeEngine({ wasm });
  await engine.handle(INIT);
  const out = [];
  await engine.handle({ type: "search", requestId: 1, query: "x" }, (m) => out.push(m));
  assert.equal(out[0].fatal, true);
  assert.equal(engine.phase, "dead");
  await engine.handle({ type: "page", requestId: 2, url: "/" }, (m) => out.push(m));
  assert.equal(out[1].fatal, true);
  await engine.handle(INIT, (m) => out.push(m));
  assert.match(out[2].message, /crashed/);
});
