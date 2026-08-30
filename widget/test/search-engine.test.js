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

// -- efficient defaults: lite-first wasm, sidecars, embedded vocab, site models --

const SPARSE = { model: "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill", tokenizer: "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill", revision: "r1", vocab_hash: "00", terms: 10 };

/** A lite/dense pair of fake modules that record which one got which call. */
function fakeVariants(manifest, opts) {
  const o = opts || {};
  const calls = [];
  const make = (variant) => ({
    capabilities: () => JSON.stringify({ dense_wasm: variant === "dense", sparse: true, version: "t" }),
    manifest: () => JSON.stringify(manifest),
    init_index: (bytes) => calls.push([variant, "init_index", bytes.length]),
    sparse_ready: () => {
      calls.push([variant, "sparse_ready"]);
      return !!o.sparseEmbedded;
    },
    init_sparse_tokenizer: (b) => calls.push([variant, "init_sparse_tokenizer", b.length]),
    attach_sidecar: (b) => {
      calls.push([variant, "attach_sidecar", b.length]);
      return JSON.stringify({ lane: "x", scopes: ["chunks"] });
    },
    init_dense_wasm: (id) => calls.push([variant, "init_dense_wasm", id]),
    search: (q, k, mode, laneId, vec) => {
      calls.push([variant, "search", laneId]);
      return JSON.stringify({ results: [], arms: { bm25: true }, degraded: [], mode, dense_lane: laneId });
    },
    chunk: (id) => JSON.stringify({ id, text: "" }),
    page: (url) => JSON.stringify({ url, chunks: [] }),
    qa_lookup: (q, laneId, vec, k) => {
      calls.push([variant, "qa_lookup", laneId]);
      return JSON.stringify([]);
    },
  });
  return { calls, lite: make("lite"), dense: make("dense") };
}

function makeVariantEngine(manifest, opts) {
  const o = opts || {};
  const posted = [];
  const v = fakeVariants(manifest, o);
  const loads = [];
  const engine = SE.createSearchEngine({
    lib,
    post: (m) => posted.push(m),
    loadWasm: async (baseUrl, version, variant) => {
      loads.push(variant);
      if (variant === "dense" && o.noDense) throw new Error("no dense module in this host");
      return variant === "dense" ? v.dense : v.lite;
    },
    loadTransformers: o.loadTransformers || (async () => { throw new Error("no transformers in tests"); }),
    canRunWebGpuLane: o.canRunWebGpuLane,
    navigator: o.navigator === undefined ? {} : o.navigator,
    indexedDB: null,
    fetch: o.fetch || fakeFetch(Object.assign({ "https://site/eddie/index.ed": new Uint8Array([1, 2, 3]) }, o.files || {})),
  });
  return { engine, posted, calls: v.calls, loads };
}

test("wasmCapabilities: capabilities() when present, feature detection for older glue", () => {
  assert.deepEqual(SE.wasmCapabilities({ capabilities: () => '{"dense_wasm":false,"sparse":true,"version":"0.4.2"}' }), { dense_wasm: false, sparse: true, version: "0.4.2" });
  assert.deepEqual(SE.wasmCapabilities({ init_dense_wasm() {} }), { dense_wasm: true, sparse: true, version: null });
  assert.deepEqual(SE.wasmCapabilities({}), { dense_wasm: false, sparse: true, version: null });
  assert.equal(SE.wasmCapabilities({ capabilities: () => "not json", init_dense_wasm() {} }).dense_wasm, true);
});

test("lite first: the lite module answers keyword searches; no dense module is fetched without a dense lane", async () => {
  const { engine, loads, calls, posted } = makeVariantEngine({ format: 5, chunks: 1, pages: 1, sections: [], dense: [], sparse: SPARSE }, { sparseEmbedded: true });
  await engine.handle(INIT);
  assert.deepEqual(loads, ["lite"]);
  const ready = posted.find((m) => m.type === "ready");
  assert.equal(ready.wasm, "lite");
  assert.equal(engine.state().wasm, "lite");
  const out = [];
  await engine.handle({ type: "search", requestId: 1, query: "x" }, (m) => out.push(m));
  assert.equal(out[0].type, "search_result");
  assert.ok(calls.some((c) => c[0] === "lite" && c[1] === "search"));
});

test("embedded sparse vocab: sparse_ready() true after init_index skips the tokenizer.json fetch", async () => {
  const fetched = [];
  const files = { "https://site/eddie/index.ed": new Uint8Array([1, 2, 3]) };
  const fetch = async (url, init) => {
    fetched.push(url);
    return fakeFetch(files)(url, init);
  };
  const { engine, calls, posted } = makeVariantEngine({ format: 5, chunks: 1, pages: 1, sections: [], dense: [], sparse: Object.assign({}, SPARSE, { vocab: "embedded" }) }, { sparseEmbedded: true, fetch });
  await engine.handle(INIT);
  const ready = posted.find((m) => m.type === "ready");
  assert.equal(ready.arms.sparse, true);
  assert.deepEqual(fetched, ["https://site/eddie/index.ed"], "only the index was fetched");
  assert.equal(calls.some((c) => c[1] === "init_sparse_tokenizer"), false);
});

test("fetched sparse vocab (0.4.1 index): tokenizer.json is downloaded and hash-checked as before", async () => {
  const tok = new TextEncoder().encode("{}");
  const hash = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"; // sha256("{}")
  const files = { "https://huggingface.co/opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill/resolve/r1/tokenizer.json": tok };
  const { engine, calls, posted } = makeVariantEngine({ format: 5, chunks: 1, pages: 1, sections: [], dense: [], sparse: Object.assign({}, SPARSE, { vocab_hash: hash }) }, { sparseEmbedded: false, files });
  await engine.handle(INIT);
  assert.equal(posted.find((m) => m.type === "ready").arms.sparse, true);
  assert.ok(calls.some((c) => c[0] === "lite" && c[1] === "init_sparse_tokenizer"));
});

const SIDECAR_MANIFEST = {
  format: 5,
  chunks: 3,
  pages: 2,
  sections: ["qa"],
  dense: [
    { id: "bge", model: "BAAI/bge-small-en-v1.5", family: "bert", dim: 4, revision: "abc", runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "model.safetensors"], base_url: "models/bge/" } },
    { id: "qwen3e", model: "Qwen/Qwen3-Embedding-0.6B", family: "qwen3", dim: 4, runtime: { kind: "webgpu-onnx", repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX", dtype: "q4" } },
  ],
  sparse: Object.assign({}, SPARSE, { vocab: "embedded" }),
  sidecars: [
    { file: "index.bge.ed", lane: "bge", scope: "qa", bytes: 40 },
    { file: "index.qwen3e.ed", lane: "qwen3e", scope: "chunks", bytes: 50 },
    { file: "index.qwen3e.ed", lane: "qwen3e", scope: "qa", bytes: 50 },
  ],
};

function siteFiles() {
  const files = { "https://site/eddie/index.ed": new Uint8Array([1, 2, 3]) };
  for (const f of ["config.json", "tokenizer.json", "model.safetensors"]) files["https://site/eddie/models/bge/" + f + "?v=1"] = new Uint8Array([9, 9]);
  files["https://site/eddie/index.bge.ed?v=1"] = new Uint8Array(40);
  files["https://site/eddie/index.qwen3e.ed?v=1"] = new Uint8Array(50);
  return files;
}

test("site-bundled wasm lane: consent names the site and the measured size; files come from next to the index with ?v=", async () => {
  const fetched = [];
  const files = siteFiles();
  const fetch = async (url, init) => {
    fetched.push((init && init.method ? init.method + " " : "") + url);
    const r = await fakeFetch(files)(url, init);
    if (init && init.method === "HEAD") return { ok: r.ok, status: r.status, headers: { get: (h) => (h === "Content-Length" ? "2" : null) } };
    return r;
  };
  const { engine, posted, loads, calls } = makeVariantEngine(SIDECAR_MANIFEST, { sparseEmbedded: true, fetch });
  await engine.handle(INIT);
  const consent = posted.find((m) => m.state === "consent_required");
  assert.equal(consent.lane.id, "bge");
  assert.equal(consent.origin, "site");
  assert.equal(consent.sizeBytes, 6, "three HEADs of 2 bytes each");
  assert.equal(consent.sidecarBytes, 0, "the wasm lane's chunk vectors are in the core file");
  assert.deepEqual(loads, ["lite"], "no dense module before consent");
  assert.ok(fetched.includes("HEAD https://site/eddie/models/bge/model.safetensors?v=1"));
  assert.equal(fetched.some((u) => /huggingface/.test(u)), false);

  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const ready = posted.find((m) => m.type === "ready");
  assert.equal(ready.lane, "bge");
  assert.equal(ready.wasm, "dense");
  assert.deepEqual(loads, ["lite", "dense"], "the dense module is fetched only for the consented CPU lane");
  assert.ok(fetched.includes("https://site/eddie/models/bge/model.safetensors?v=1"));
  // Hand-over: the dense module got the index again and then the lane.
  const dense = calls.filter((c) => c[0] === "dense").map((c) => c[1]);
  assert.deepEqual(dense.slice(0, 2), ["init_index", "init_dense_wasm"]);
  assert.equal(engine.state().wasm, "dense");
  // The qa sidecar of the active lane is fetched by the first QA lookup only.
  assert.equal(fetched.some((u) => /index\.bge\.ed/.test(u)), false);
  const out = [];
  await engine.handle({ type: "search", requestId: 1, query: "why?", qa: 2 }, (m) => out.push(m));
  assert.ok(fetched.includes("https://site/eddie/index.bge.ed?v=1"));
  assert.ok(calls.some((c) => c[0] === "dense" && c[1] === "attach_sidecar" && c[2] === 40));
  await engine.handle({ type: "qa", requestId: 2, query: "why?" }, (m) => out.push(m));
  assert.equal(fetched.filter((u) => /index\.bge\.ed/.test(u)).length, 1, "attached once");
});

test("a manifest that declares the bundle size skips the HEAD probes", async () => {
  const manifest = JSON.parse(JSON.stringify(SIDECAR_MANIFEST));
  manifest.dense[0].runtime.bytes = 67458275;
  const heads = [];
  const fetch = async (url, init) => {
    if (init && init.method === "HEAD") heads.push(url);
    return fakeFetch(siteFiles())(url, init);
  };
  const { engine, posted } = makeVariantEngine(manifest, { sparseEmbedded: true, fetch });
  await engine.handle(INIT);
  const consent = posted.find((m) => m.state === "consent_required");
  assert.equal(consent.sizeBytes, 67458275);
  assert.deepEqual(heads, []);
});

test("evictStaleFiles keeps @site keys of the index's lanes and drops other repos", async () => {
  const store = new Map([
    ["BAAI/bge-small-en-v1.5@abc/@site/model.safetensors", 1],
    ["BAAI/bge-small-en-v1.5@abc/config.json", 1],
    ["some/old-model@main/model.safetensors", 1],
    ["url:https://site/eddie/models/x/onnx/model.onnx", 1],
  ]);
  const req = (result) => {
    const r = { result };
    setImmediate(() => r.onsuccess && r.onsuccess());
    return r;
  };
  const fakeIdb = {
    open: () => {
      const r = {
        result: {
          objectStoreNames: { contains: () => true },
          transaction: () => ({
            objectStore: () => ({
              get: (k) => req(store.get(k)),
              getKey: (k) => req(store.has(k) ? k : undefined),
              put: (v, k) => req(store.set(k, v)),
              delete: (k) => req(store.delete(k)),
              getAllKeys: () => req(Array.from(store.keys())),
            }),
          }),
        },
      };
      setImmediate(() => r.onsuccess && r.onsuccess());
      return r;
    },
  };
  const posted = [];
  const v = fakeVariants({ format: 5, chunks: 1, pages: 1, sections: [], dense: [SIDECAR_MANIFEST.dense[0]], sparse: Object.assign({}, SPARSE, { vocab: "embedded" }) }, { sparseEmbedded: true });
  const engine = SE.createSearchEngine({
    lib,
    post: (m) => posted.push(m),
    loadWasm: async () => v.lite,
    loadTransformers: async () => { throw new Error("no transformers in tests"); },
    navigator: {},
    indexedDB: fakeIdb,
    fetch: fakeFetch(siteFiles()),
  });
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  assert.equal(posted.some((m) => m.type === "ready"), true);
  await new Promise((r) => setTimeout(r, 20)); // evictStaleFiles runs after ready
  assert.ok(store.has("BAAI/bge-small-en-v1.5@abc/@site/model.safetensors"), "site bundle key kept");
  assert.ok(store.has("url:https://site/eddie/models/x/onnx/model.onnx"), "url: keys kept");
  assert.equal(store.has("some/old-model@main/model.safetensors"), false, "stale repo evicted");
});

test("a host without the dense module degrades the CPU lane instead of failing init", async () => {
  const { engine, posted, loads } = makeVariantEngine(SIDECAR_MANIFEST, { sparseEmbedded: true, files: siteFiles(), noDense: true });
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const ready = posted.find((m) => m.type === "ready");
  assert.deepEqual(loads, ["lite", "dense"]);
  assert.equal(ready.lane, null);
  assert.equal(ready.wasm, "lite");
  assert.ok(ready.degraded.some((d) => /lane bge failed: no dense module/.test(d)));
});

test("webgpu lane: its chunk sidecar is fetched before the model, and transformers.js is pointed at the site copy when base_url says so", async () => {
  const manifest = JSON.parse(JSON.stringify(SIDECAR_MANIFEST));
  manifest.dense[1].runtime.base_url = "models/qwen3e/";
  const fetched = [];
  const files = siteFiles();
  const fetch = async (url, init) => {
    fetched.push(url);
    return fakeFetch(files)(url, init);
  };
  const tfEnv = { backends: { onnx: {} } };
  const tf = {
    env: tfEnv,
    pipeline: async (task, repo, opts) => {
      tf.pipelineArgs = [task, repo, opts];
      return async () => ({ data: new Float32Array([1, 0, 0, 0]) });
    },
  };
  const navigator = { gpu: { requestAdapter: async () => ({ features: new Set(), limits: { maxBufferSize: 1 } }) } };
  const { engine, posted, loads, calls } = makeVariantEngine(manifest, { sparseEmbedded: true, fetch, navigator, canRunWebGpuLane: true, loadTransformers: async () => tf });
  await engine.handle(INIT);
  const consent = posted.find((m) => m.state === "consent_required");
  assert.equal(consent.lane.id, "qwen3e");
  assert.equal(consent.sidecarBytes, 50);
  assert.equal(consent.origin, "site");
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const ready = posted.find((m) => m.type === "ready");
  assert.equal(ready.lane, "qwen3e");
  assert.equal(ready.runtime, "webgpu");
  assert.deepEqual(loads, ["lite"], "a WebGPU lane never needs the dense module");
  assert.ok(fetched.includes("https://site/eddie/index.qwen3e.ed?v=1"));
  assert.ok(calls.some((c) => c[0] === "lite" && c[1] === "attach_sidecar" && c[2] === 50));
  assert.equal(tfEnv.remoteHost, "https://site/eddie/models/qwen3e/");
  assert.equal(tfEnv.remotePathTemplate, ".");
  assert.equal(tf.pipelineArgs[1], "onnx-community/Qwen3-Embedding-0.6B-ONNX", "the model id stays valid for transformers.js");
  const out = [];
  await engine.handle({ type: "search", requestId: 1, query: "q" }, (m) => out.push(m));
  assert.equal(out[0].lane, "qwen3e");
  // qa scope shares the lane's sidecar file: nothing more to fetch for QA.
  await engine.handle({ type: "qa", requestId: 2, query: "why?" }, (m) => out.push(m));
  assert.equal(fetched.filter((u) => /index\.qwen3e\.ed/.test(u)).length, 1);
});

test("webgpu lane without base_url keeps transformers.js on huggingface.co", async () => {
  const tfEnv = {};
  const tf = { env: tfEnv, pipeline: async () => async () => ({ data: new Float32Array([1, 0, 0, 0]) }) };
  const navigator = { gpu: { requestAdapter: async () => ({ features: new Set(), limits: { maxBufferSize: 1 } }) } };
  const { engine, posted } = makeVariantEngine(SIDECAR_MANIFEST, { sparseEmbedded: true, files: siteFiles(), navigator, canRunWebGpuLane: true, loadTransformers: async () => tf });
  await engine.handle(INIT);
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  assert.equal(posted.find((m) => m.type === "ready").lane, "qwen3e");
  assert.equal(tfEnv.remoteHost, "https://huggingface.co/");
  assert.equal(tfEnv.remotePathTemplate, "{model}/resolve/{revision}/");
});

test("a host that names another tier for the lane's runtime gets a tier_required status instead of a degraded lane", async () => {
  const posted = [];
  const v = fakeVariants(SIDECAR_MANIFEST, { sparseEmbedded: true });
  const navigator = { gpu: { requestAdapter: async () => ({ features: new Set(), limits: { maxBufferSize: 1 } }) } };
  const engine = SE.createSearchEngine({
    lib,
    post: (m) => posted.push(m),
    loadWasm: async () => v.lite,
    loadTransformers: async () => {
      const e = new Error("the lite service worker has no transformers.js; the gpu tier hosts it");
      e.eddieTier = "gpu";
      throw e;
    },
    canRunWebGpuLane: true,
    navigator,
    indexedDB: null,
    fetch: fakeFetch(siteFiles()),
  });
  await engine.handle(INIT);
  assert.equal(posted.find((m) => m.state === "consent_required").lane.id, "qwen3e", "the lite host still chooses the webgpu lane");
  await engine.handle(Object.assign({}, INIT, { consent: true }));
  const tier = posted.find((m) => m.state === "tier_required");
  assert.ok(tier, "tier_required posted");
  assert.equal(tier.tier, "gpu");
  assert.equal(tier.lane.id, "qwen3e");
  assert.equal(engine.phase, "awaiting_tier");
  assert.equal(posted.some((m) => m.type === "ready"), false, "no keyword-only ready: the widget re-inits on the gpu tier");
});
