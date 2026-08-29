// SPDX-License-Identifier: GPL-3.0-only

// Eddie search worker (classic worker).
//
// widget/build.sh concatenates widget/src/lib/*.js ahead of this file, so the
// pure helpers are available as `EddieLib`. The WASM glue is loaded with
// importScripts; transformers.js (WebGPU dense lane) with a dynamic import().
//
// Protocol (main thread -> worker):
//   init        {indexUrl, baseUrl, version?, denseRuntime?, consent?}
//   cache_check {requestId}
//   search      {requestId, query, topK?, mode?, evidence?, qa?}
//   page        {requestId, url}
//   chunk       {requestId, id}
//   qa          {requestId, query, k?}
// (worker -> main thread):
//   status        {state, ...}   loading_wasm | loading_index | index_ready |
//                                consent_required | downloading_model |
//                                loading_model | error
//   ready         {lanes, lane, arms, degraded, manifest}
//   cache_result  {requestId, cached, lane, sizeBytes}
//   search_result {requestId, results, arms, degraded, mode, qa?}
//   page_result   {requestId, page}
//   chunk_result  {requestId, chunk}
//   qa_result     {requestId, hits}
//   error         {requestId?, message, fatal?, unsupported?}

"use strict";

const IDB_NAME = "eddie-models";
const IDB_VERSION = 2;
const IDB_FILES = "files";
const IDB_META = "meta";
const TRANSFORMERS_URL = "https://cdn.jsdelivr.net/npm/@huggingface/transformers@4.2.0";
const PROGRESS_INTERVAL_MS = 80;

const lib = EddieLib;

const state = {
  phase: "idle", // idle | loading | awaiting_consent | ready | error | dead
  initRunning: false,
  rerun: false,
  baseUrl: "",
  version: null,
  indexUrl: null,
  denseRuntime: "auto",
  consent: Object.create(null), // lane id -> true
  wasmReady: false,
  indexLoaded: false,
  manifest: null,
  candidates: [],
  candidateReason: null,
  laneIndex: 0,
  dense: null, // { lane, kind: "wasm" | "webgpu", embed? }
  sparse: false,
  degraded: [],
  gpu: null, // { adapter, hasF16, maxBufferSize } | false
  db: undefined, // IDBDatabase | null (unavailable)
};

self.onmessage = function (e) {
  const msg = e.data || {};
  switch (msg.type) {
    case "init":
      handleInit(msg);
      break;
    case "cache_check":
      handleCacheCheck(msg);
      break;
    case "search":
      handleSearch(msg);
      break;
    case "page":
      handleLookup(msg, "page_result", "page", () => wasm_bindgen.page(String(msg.url)));
      break;
    case "chunk":
      handleLookup(msg, "chunk_result", "chunk", () => wasm_bindgen.chunk(Number(msg.id)));
      break;
    case "qa":
      handleQa(msg);
      break;
    default:
      postError(msg.requestId, `unknown message type ${String(msg.type)}`);
  }
};

// -- init ---------------------------------------------------------------

function handleInit(msg) {
  if (state.phase === "dead") {
    postError(null, "search engine crashed; reload the page to retry", true);
    return;
  }
  if (msg.indexUrl) state.indexUrl = String(msg.indexUrl);
  if (msg.baseUrl != null) state.baseUrl = String(msg.baseUrl);
  if (msg.version != null) state.version = msg.version ? String(msg.version) : null;
  if (msg.denseRuntime) state.denseRuntime = String(msg.denseRuntime);
  if (msg.consent === true) {
    const lane = currentCandidate();
    if (lane) state.consent[lane.id] = true;
    if (msg.consentLane) state.consent[String(msg.consentLane)] = true;
  }
  runInit();
}

async function runInit() {
  if (state.initRunning) {
    state.rerun = true;
    return;
  }
  state.initRunning = true;
  try {
    state.phase = "loading";
    await ensureWasm();
    await ensureIndex();
    await ensureDb();
    await ensureGpu();
    await ensureSparse();
    const outcome = await ensureDense();
    if (outcome === "consent") {
      state.phase = "awaiting_consent";
      return;
    }
    state.phase = "ready";
    postReady();
    evictStaleFiles();
  } catch (err) {
    state.phase = isFatal(err) ? "dead" : "error";
    postStatus("error", {
      message: describe(err),
      fatal: isFatal(err),
      unsupported: isUnsupported(err),
    });
  } finally {
    state.initRunning = false;
    if (state.rerun) {
      state.rerun = false;
      runInit();
    }
  }
}

async function ensureWasm() {
  if (state.wasmReady) return;
  postStatus("loading_wasm");
  if (typeof WebAssembly !== "object") {
    throw unsupported("This browser can't run WebAssembly.");
  }
  try {
    importScripts(lib.assetUrl(state.baseUrl, "eddie-wasm.js", state.version));
    await wasm_bindgen({ module_or_path: lib.assetUrl(state.baseUrl, "eddie.wasm", state.version) });
  } catch (err) {
    if (err instanceof WebAssembly.CompileError || err instanceof WebAssembly.LinkError) {
      throw unsupported("This browser can't run the search engine (WebAssembly SIMD is required).");
    }
    throw err;
  }
  state.wasmReady = true;
}

async function ensureIndex() {
  if (state.indexLoaded) return;
  if (!state.indexUrl) throw new Error("init: indexUrl is required");
  postStatus("loading_index", { progress: null });
  let last = 0;
  const bytes = await lib.fetchBytes(state.indexUrl, {
    timeoutMs: 180000,
    onProgress: (loaded, total) => {
      const now = Date.now();
      if (total && now - last < PROGRESS_INTERVAL_MS && loaded !== total) return;
      last = now;
      postStatus("loading_index", { progress: total ? loaded / total : null, loaded, total });
    },
  });
  state.manifest = JSON.parse(wasm_bindgen.manifest(bytes));
  wasm_bindgen.init_index(bytes);
  state.indexLoaded = true;
  state.dense = null;
  state.sparse = false;
  state.degraded = [];
  state.laneIndex = 0;
  postStatus("index_ready", { manifest: manifestSummary(), lanes: laneList() });
}

async function ensureGpu() {
  if (state.gpu !== null) return;
  state.gpu = false;
  try {
    if (state.denseRuntime !== "wasm" && self.navigator && navigator.gpu && typeof navigator.gpu.requestAdapter === "function") {
      const adapter = await navigator.gpu.requestAdapter();
      if (adapter) {
        state.gpu = {
          adapter,
          hasF16: !!(adapter.features && adapter.features.has("shader-f16")),
          maxBufferSize: adapter.limits ? adapter.limits.maxBufferSize : 0,
        };
      }
    }
  } catch (err) {
    console.warn("eddie: WebGPU adapter probe failed", err);
    state.gpu = false;
  }
  const choice = lib.chooseDenseLanes(state.manifest, {
    denseRuntime: state.denseRuntime,
    hasWebGpu: !!state.gpu,
  });
  state.candidates = choice.candidates;
  state.candidateReason = choice.reason;
}

async function ensureSparse() {
  if (state.sparse) return;
  const spec = state.manifest && state.manifest.sparse;
  if (!spec) return;
  if (state.degraded.some((d) => d.startsWith("sparse:"))) return;
  try {
    const repo = spec.tokenizer || spec.model;
    const rev = spec.revision || "main";
    let bytes = await getModelFile(repo, rev, "tokenizer.json", "sparse");
    let ok = await lib.verifySha256(bytes, spec.vocab_hash);
    if (!ok && bytes.fromCache) {
      await idbDelete(lib.cacheKey(repo, rev, "tokenizer.json"));
      bytes = await getModelFile(repo, rev, "tokenizer.json", "sparse", { noCache: true });
      ok = await lib.verifySha256(bytes, spec.vocab_hash);
    }
    if (!ok) {
      throw new Error("tokenizer.json SHA-256 does not match the index's vocab_hash");
    }
    wasm_bindgen.init_sparse_tokenizer(bytes);
    state.sparse = true;
  } catch (err) {
    if (isFatal(err)) throw err;
    console.warn("eddie: sparse arm unavailable", err);
    state.degraded.push(`sparse: ${describe(err)}`);
  }
}

/** Returns "ok" (a lane is loaded), "consent" (waiting for the visitor) or "none". */
async function ensureDense() {
  if (state.dense) return "ok";
  for (; state.laneIndex < state.candidates.length; state.laneIndex++) {
    const lane = state.candidates[state.laneIndex];
    try {
      const cached = await laneCached(lane);
      if (!cached && !state.consent[lane.id]) {
        postStatus("consent_required", {
          lane: laneSummary(lane),
          sizeBytes: lib.laneDownloadBytes(lane),
          saveData: !!(self.navigator && navigator.connection && navigator.connection.saveData),
        });
        return "consent";
      }
      if (lib.isWasmLane(lane)) {
        await loadWasmLane(lane);
      } else {
        await loadWebGpuLane(lane);
      }
      return "ok";
    } catch (err) {
      if (isFatal(err)) throw err;
      console.warn(`eddie: dense lane ${lane.id} failed`, err);
      state.degraded.push(`dense: lane ${lane.id} failed: ${describe(err)}`);
    }
  }
  if (state.candidates.length === 0 && state.candidateReason && (state.manifest.dense || []).length > 0) {
    if (!state.degraded.some((d) => d.startsWith("dense:"))) {
      state.degraded.push(`dense: ${state.candidateReason}`);
    }
  }
  return "none";
}

async function loadWasmLane(lane) {
  const repo = lib.laneRepo(lane);
  const rev = lib.laneRevision(lane);
  const files = lib.laneFiles(lane);
  const loaded = {};
  for (const file of files) {
    loaded[file] = await getModelFile(repo, rev, file, lane.id);
  }
  postStatus("loading_model", { lane: laneSummary(lane) });
  const config = loaded["config.json"];
  const tokenizer = loaded["tokenizer.json"];
  const weights = files.map((f) => loaded[f]).find((b, i) => lib.isWeightsFile(files[i]));
  if (!config || !tokenizer || !weights) {
    throw new Error(`lane ${lane.id}: runtime.files must include config.json, tokenizer.json and a weights file`);
  }
  wasm_bindgen.init_dense_wasm(lane.id, config, tokenizer, weights);
  state.dense = { lane, kind: "wasm" };
}

async function loadWebGpuLane(lane) {
  const runtime = lane.runtime;
  postStatus("loading_model", { lane: laneSummary(lane), file: "transformers.js" });
  const tf = await import(TRANSFORMERS_URL);
  const dtype = lib.pickDtype(runtime, state.gpu && state.gpu.hasF16);
  let last = 0;
  const extractor = await tf.pipeline("feature-extraction", runtime.repo, {
    device: "webgpu",
    dtype,
    revision: lib.laneRevision(lane),
    progress_callback: (p) => {
      if (!p || p.status !== "progress") return;
      const now = Date.now();
      if (now - last < PROGRESS_INTERVAL_MS && p.progress < 100) return;
      last = now;
      postStatus("downloading_model", {
        lane: laneSummary(lane),
        file: p.file || p.name || "",
        progress: typeof p.progress === "number" ? p.progress / 100 : null,
        loaded: p.loaded,
        total: p.total,
      });
    },
  });
  postStatus("loading_model", { lane: laneSummary(lane) });
  const pooling = runtime.pooling || "mean";
  const embed = async (text) => {
    const prefixed = (lane.query_prefix || "") + text;
    const out = await extractor(prefixed, { pooling, normalize: lane.normalize !== false });
    const data = out.data instanceof Float32Array ? out.data : Float32Array.from(out.data);
    if (typeof out.dispose === "function") out.dispose();
    if (data.length !== lane.dim) {
      throw new Error(`lane ${lane.id}: model produced ${data.length}-d vectors but the index stores ${lane.dim}-d`);
    }
    return data;
  };
  await embed("warm up");
  state.dense = { lane, kind: "webgpu", embed, dtype };
  await idbMetaPut(webGpuMarker(lane, dtype), Date.now());
}

function postReady() {
  self.postMessage({
    type: "ready",
    lanes: laneList(),
    lane: state.dense ? state.dense.lane.id : null,
    runtime: state.dense ? state.dense.kind : null,
    arms: { dense: !!state.dense, sparse: state.sparse, bm25: true },
    degraded: lib.filterDesignDegraded(unique(state.degraded)),
    manifest: manifestSummary(),
  });
}

// -- cache check --------------------------------------------------------

async function handleCacheCheck(msg) {
  try {
    await ensureWasm();
    await ensureIndex();
    await ensureDb();
    await ensureGpu();
    const lane = currentCandidate();
    const cached = lane ? await laneCached(lane) : true;
    self.postMessage({
      type: "cache_result",
      requestId: msg.requestId,
      cached,
      lane: lane ? laneSummary(lane) : null,
      sizeBytes: lane ? lib.laneDownloadBytes(lane) : 0,
    });
  } catch (err) {
    postError(msg.requestId, describe(err), isFatal(err));
  }
}

async function laneCached(lane) {
  if (lib.isWasmLane(lane)) {
    if (!state.db) return false;
    const repo = lib.laneRepo(lane);
    const rev = lib.laneRevision(lane);
    for (const file of lib.laneFiles(lane)) {
      if (!(await idbHas(lib.cacheKey(repo, rev, file)))) return false;
    }
    return true;
  }
  const dtype = lib.pickDtype(lane.runtime, state.gpu && state.gpu.hasF16);
  return !!(await idbMetaGet(webGpuMarker(lane, dtype)));
}

function webGpuMarker(lane, dtype) {
  return `webgpu:${lane.runtime.repo}@${lib.laneRevision(lane)}#${dtype}`;
}

// -- queries ------------------------------------------------------------

async function queryVector(query, mode) {
  if (!state.dense || state.dense.kind !== "webgpu") return { laneId: null, vec: null };
  if (mode !== "hybrid" && mode !== "dense") return { laneId: null, vec: null };
  const vec = await state.dense.embed(query);
  return { laneId: state.dense.lane.id, vec };
}

async function handleSearch(msg) {
  const requestId = msg.requestId;
  try {
    requireIndex();
    const query = String(msg.query || "").trim();
    if (!query) throw new Error("search: empty query");
    const mode = msg.mode || "hybrid";
    const topK = Number(msg.topK) > 0 ? Number(msg.topK) : 8;
    const { laneId, vec } = await queryVector(query, mode);
    const res = JSON.parse(wasm_bindgen.search(query, topK, mode, laneId, vec));
    const results = res.results || [];
    if (msg.evidence) {
      for (const r of results) {
        try {
          r.text = JSON.parse(wasm_bindgen.chunk(r.chunk)).text;
        } catch (_) {
          r.text = r.snippet;
        }
      }
    }
    let qa = undefined;
    if (msg.qa && Number(msg.qa) > 0 && hasSection("qa")) {
      qa = JSON.parse(wasm_bindgen.qa_lookup(query, laneId, vec, Number(msg.qa)));
    }
    self.postMessage({
      type: "search_result",
      requestId,
      results,
      arms: res.arms,
      degraded: lib.filterDesignDegraded(unique((res.degraded || []).concat(state.degraded))),
      mode: res.mode,
      lane: res.dense_lane || null,
      qa,
    });
  } catch (err) {
    failRequest(requestId, err);
  }
}

function handleLookup(msg, type, key, fn) {
  try {
    requireIndex();
    self.postMessage({ type, requestId: msg.requestId, [key]: JSON.parse(fn()) });
  } catch (err) {
    failRequest(msg.requestId, err);
  }
}

async function handleQa(msg) {
  try {
    requireIndex();
    const query = String(msg.query || "").trim();
    const k = Number(msg.k) > 0 ? Number(msg.k) : 3;
    if (!query || !hasSection("qa")) {
      self.postMessage({ type: "qa_result", requestId: msg.requestId, hits: [] });
      return;
    }
    const { laneId, vec } = await queryVector(query, "dense");
    const hits = JSON.parse(wasm_bindgen.qa_lookup(query, laneId, vec, k));
    self.postMessage({ type: "qa_result", requestId: msg.requestId, hits });
  } catch (err) {
    failRequest(msg.requestId, err);
  }
}

function failRequest(requestId, err) {
  if (isFatal(err)) {
    // A trapped panic leaves the WASM instance unusable; never re-enter it.
    state.phase = "dead";
    postError(requestId, "search engine crashed: " + describe(err), true);
    return;
  }
  postError(requestId, describe(err), false);
}

function requireIndex() {
  if (state.phase === "dead") throw fatal("search engine crashed; reload to retry");
  if (!state.indexLoaded) throw new Error("index not loaded yet");
}

function hasSection(name) {
  return !!(state.manifest && Array.isArray(state.manifest.sections) && state.manifest.sections.includes(name));
}

// -- model files --------------------------------------------------------

async function getModelFile(repo, rev, file, laneId, opts) {
  const key = lib.cacheKey(repo, rev, file);
  if (!(opts && opts.noCache)) {
    const cached = await idbGet(key);
    if (cached) {
      const bytes = new Uint8Array(cached);
      bytes.fromCache = true;
      return bytes;
    }
  }
  const url = lib.hfFileUrl(repo, rev, file);
  let last = 0;
  postStatus("downloading_model", { lane: laneId, file, progress: null, loaded: 0, total: null });
  const bytes = await lib.fetchBytes(url, {
    timeoutMs: lib.timeoutForFile(file),
    retries: 1,
    backoffMs: 1500,
    onProgress: (loaded, total) => {
      const now = Date.now();
      if (now - last < PROGRESS_INTERVAL_MS && !(total && loaded === total)) return;
      last = now;
      postStatus("downloading_model", { lane: laneId, file, progress: total ? loaded / total : null, loaded, total });
    },
  });
  await idbPut(key, bytes.buffer);
  return bytes;
}

// -- IndexedDB (best effort: any failure only logs) --------------------

async function ensureDb() {
  if (state.db !== undefined) return;
  state.db = null;
  if (typeof indexedDB === "undefined") return;
  try {
    state.db = await new Promise((resolve, reject) => {
      const req = indexedDB.open(IDB_NAME, IDB_VERSION);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(IDB_FILES)) db.createObjectStore(IDB_FILES);
        if (!db.objectStoreNames.contains(IDB_META)) db.createObjectStore(IDB_META);
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error || new Error("indexedDB.open failed"));
      req.onblocked = () => reject(new Error("indexedDB.open blocked"));
    });
  } catch (err) {
    console.warn("eddie: model cache unavailable; files will be re-downloaded next visit", err);
    state.db = null;
  }
}

function idbRequest(store, mode, fn) {
  if (!state.db) return Promise.resolve(undefined);
  return new Promise((resolve) => {
    try {
      const tx = state.db.transaction(store, mode);
      const req = fn(tx.objectStore(store));
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => {
        console.warn("eddie: model cache operation failed", req.error);
        resolve(undefined);
      };
    } catch (err) {
      console.warn("eddie: model cache operation failed", err);
      resolve(undefined);
    }
  });
}

const idbGet = (key) => idbRequest(IDB_FILES, "readonly", (s) => s.get(key));
const idbHas = async (key) => (await idbRequest(IDB_FILES, "readonly", (s) => s.getKey(key))) !== undefined;
const idbPut = (key, value) => idbRequest(IDB_FILES, "readwrite", (s) => s.put(value, key));
const idbDelete = (key) => idbRequest(IDB_FILES, "readwrite", (s) => s.delete(key));
const idbMetaGet = (key) => idbRequest(IDB_META, "readonly", (s) => s.get(key));
const idbMetaPut = (key, value) => idbRequest(IDB_META, "readwrite", (s) => s.put(value, key));

/** Delete cached files that belong to models this index no longer references. */
async function evictStaleFiles() {
  if (!state.db || !state.manifest) return;
  const keep = [];
  for (const lane of state.manifest.dense || []) {
    if (lib.isWasmLane(lane)) keep.push(`${lib.laneRepo(lane)}@${lib.laneRevision(lane)}/`);
  }
  if (state.manifest.sparse) {
    const s = state.manifest.sparse;
    keep.push(`${s.tokenizer || s.model}@${s.revision || "main"}/`);
  }
  const keys = await idbRequest(IDB_FILES, "readonly", (s) => s.getAllKeys());
  for (const key of keys || []) {
    if (typeof key === "string" && !keep.some((p) => key.startsWith(p))) {
      console.info("eddie: evicting cached model file", key);
      await idbDelete(key);
    }
  }
}

// -- helpers ------------------------------------------------------------

function currentCandidate() {
  return state.candidates[state.laneIndex] || null;
}

function laneSummary(lane) {
  return {
    id: lane.id,
    model: lane.model,
    kind: lane.runtime ? lane.runtime.kind : null,
    repo: lib.laneRepo(lane),
    dim: lane.dim,
  };
}

function laneList() {
  return (state.manifest && state.manifest.dense ? state.manifest.dense : []).map(laneSummary);
}

function manifestSummary() {
  const m = state.manifest || {};
  return {
    format: m.format,
    eddie: m.eddie,
    chunks: m.chunks,
    pages: m.pages,
    sections: m.sections || [],
    sparse: !!m.sparse,
  };
}

function unique(list) {
  return Array.from(new Set(list));
}

function postStatus(stateName, extra) {
  self.postMessage(Object.assign({ type: "status", state: stateName }, extra || {}));
}

function postError(requestId, message, isFatalErr, isUnsupported) {
  self.postMessage({ type: "error", requestId: requestId == null ? undefined : requestId, message, fatal: !!isFatalErr, unsupported: !!isUnsupported });
}

function describe(err) {
  if (err == null) return "unknown error";
  if (typeof err === "string") return err;
  if (err.message) return err.message;
  return String(err);
}

function unsupported(message) {
  const e = new Error(message);
  e.eddieUnsupported = true;
  e.eddieFatal = true;
  return e;
}

function fatal(message) {
  const e = new Error(message);
  e.eddieFatal = true;
  return e;
}

function isUnsupported(err) {
  return !!(err && err.eddieUnsupported);
}

function isFatal(err) {
  if (!err) return false;
  if (err.eddieFatal) return true;
  return typeof WebAssembly === "object" && err instanceof WebAssembly.RuntimeError;
}
