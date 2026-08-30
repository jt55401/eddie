// SPDX-License-Identifier: GPL-3.0-only

// Eddie search engine, host-independent.
//
// Everything the search worker does (manifest, lane choice, downloads with
// timeouts and retry, IndexedDB cache, sparse tokenizer hash check, the
// transformers.js WebGPU lane, the WASM calls and the consent handshake)
// lives here, behind an `env` the host supplies:
//
//   createSearchEngine({
//     post(message)                 broadcast sink: status and ready events
//     loadWasm(baseUrl, version)    -> Promise of the wasm-bindgen API object
//     loadTransformers()            -> Promise of the transformers.js module
//     canRunWebGpuLane              false when this host cannot run webgpu-onnx lanes
//     navigator, indexedDB          optional overrides (tests)
//   })
//
// `engine.handle(msg, reply)` dispatches one protocol message; `reply` is
// the sink for that message's own answers (search_result, cache_result,
// error with requestId ...). In a dedicated worker `post` and `reply` are
// both `self.postMessage`; in the service worker `reply` is the requesting
// page's port and `post` fans out to every connected page.
//
// The message shapes are documented in widget/README.md ("Worker protocol").

(function (factory) {
  const api = factory();
  if (typeof module === "object" && module && module.exports) {
    module.exports = api;
  } else if (typeof EddieLib === "object" && EddieLib) {
    Object.assign(EddieLib, api);
  } else {
    globalThis.EddieLib = Object.assign(globalThis.EddieLib || {}, api);
  }
})(function () {
  "use strict";

  const IDB_NAME = "eddie-models";
  const IDB_VERSION = 2;
  const IDB_FILES = "files";
  const IDB_META = "meta";
  const PROGRESS_INTERVAL_MS = 80;
  const NOT_LOADED = "index not loaded yet";

  function createSearchEngine(env) {
    const lib = typeof EddieLib === "object" && EddieLib ? EddieLib : env.lib;
    const post = env.post;
    const nav = env.navigator !== undefined ? env.navigator : globalThis.navigator;
    const idb = env.indexedDB !== undefined ? env.indexedDB : globalThis.indexedDB;
    const canRunWebGpuLane = env.canRunWebGpuLane !== false;

    let wasm = null; // wasm-bindgen API object once loaded

    const state = {
      phase: "idle", // idle | loading | awaiting_consent | ready | error | dead
      initRunning: false,
      rerun: false,
      baseUrl: "",
      version: null,
      indexUrl: null,
      loadedIndexUrl: null,
      denseRuntime: "auto",
      consent: Object.create(null), // lane id -> true
      wasmReady: false,
      indexLoaded: false,
      manifest: null,
      candidates: [],
      candidateReason: null,
      hostSkipped: [], // webgpu-onnx lane ids this host cannot run
      laneIndex: 0,
      dense: null, // { lane, kind: "wasm" | "webgpu", embed? }
      sparse: false,
      degraded: [],
      gpu: null, // { adapter, hasF16, maxBufferSize } | false
      db: undefined, // IDBDatabase | null (unavailable)
    };

    // -- dispatch ---------------------------------------------------------

    function handle(msg, reply) {
      const m = msg || {};
      const out = reply || post;
      switch (m.type) {
        case "init":
          return handleInit(m, out);
        case "cache_check":
          return handleCacheCheck(m, out);
        case "search":
          return handleSearch(m, out);
        case "page":
          return handleLookup(m, out, "page_result", "page", () => wasm.page(String(m.url)));
        case "chunk":
          return handleLookup(m, out, "chunk_result", "chunk", () => wasm.chunk(Number(m.id)));
        case "qa":
          return handleQa(m, out);
        default:
          postError(out, m.requestId, `unknown message type ${String(m.type)}`);
          return Promise.resolve();
      }
    }

    /** Snapshot for the service worker's `state` reply. */
    function snapshot() {
      return {
        phase: state.phase,
        indexUrl: state.loadedIndexUrl,
        version: state.version,
        indexLoaded: state.indexLoaded,
        lane: state.dense ? state.dense.lane.id : null,
        runtime: state.dense ? state.dense.kind : null,
        arms: state.indexLoaded ? { dense: !!state.dense, sparse: state.sparse, bm25: true } : null,
        degraded: lib.filterDesignDegraded(unique(state.degraded)),
        manifest: state.manifest ? manifestSummary() : null,
        lanes: laneList(),
        hostSkippedLanes: state.hostSkipped.slice(),
      };
    }

    // -- init ---------------------------------------------------------------

    function handleInit(msg, reply) {
      if (state.phase === "dead") {
        postError(reply, null, "search engine crashed; reload the page to retry", true);
        return Promise.resolve();
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
      return runInit();
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
        wasm = await env.loadWasm(state.baseUrl, state.version);
      } catch (err) {
        if (typeof WebAssembly === "object" && (err instanceof WebAssembly.CompileError || err instanceof WebAssembly.LinkError)) {
          throw unsupported("This browser can't run the search engine (WebAssembly SIMD is required).");
        }
        throw err;
      }
      if (!wasm || typeof wasm.search !== "function") {
        throw new Error("loadWasm did not return the Eddie WASM API");
      }
      state.wasmReady = true;
    }

    async function ensureIndex() {
      if (!state.indexUrl) throw new Error("init: indexUrl is required");
      if (state.indexLoaded && state.loadedIndexUrl === state.indexUrl) return;
      postStatus("loading_index", { progress: null });
      let last = 0;
      const bytes = await lib.fetchBytes(state.indexUrl, {
        fetch: env.fetch,
        timeoutMs: 180000,
        onProgress: (loaded, total) => {
          const now = Date.now();
          if (total && now - last < PROGRESS_INTERVAL_MS && loaded !== total) return;
          last = now;
          postStatus("loading_index", { progress: total ? loaded / total : null, loaded, total });
        },
      });
      state.manifest = JSON.parse(wasm.manifest(bytes));
      wasm.init_index(bytes);
      state.indexLoaded = true;
      state.loadedIndexUrl = state.indexUrl;
      state.dense = null;
      state.sparse = false;
      state.degraded = [];
      state.hostSkipped = [];
      state.laneIndex = 0;
      state.gpu = null;
      postStatus("index_ready", { manifest: manifestSummary(), lanes: laneList() });
    }

    async function ensureGpu() {
      if (state.gpu !== null) return;
      state.gpu = false;
      try {
        if (state.denseRuntime !== "wasm" && nav && nav.gpu && typeof nav.gpu.requestAdapter === "function") {
          const adapter = await nav.gpu.requestAdapter();
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
        hasWebGpu: !!state.gpu && canRunWebGpuLane,
      });
      state.candidates = choice.candidates;
      state.candidateReason = choice.reason;
      if (!canRunWebGpuLane) {
        state.hostSkipped = (state.manifest.dense || []).filter(lib.isWebGpuLane).map((l) => l.id);
      }
      for (const { lane, reason } of choice.skipped) {
        console.warn(`eddie: dense lane ${lane.id} skipped: ${reason}`);
        state.degraded.push(`dense: lane ${lane.id} skipped: ${reason}`);
      }
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
        let ok = await lib.verifySha256(bytes, spec.vocab_hash, env.subtle);
        if (!ok && bytes.fromCache) {
          await idbDelete(lib.cacheKey(repo, rev, "tokenizer.json"));
          bytes = await getModelFile(repo, rev, "tokenizer.json", "sparse", { noCache: true });
          ok = await lib.verifySha256(bytes, spec.vocab_hash, env.subtle);
        }
        if (!ok) {
          throw new Error("tokenizer.json SHA-256 does not match the index's vocab_hash");
        }
        wasm.init_sparse_tokenizer(bytes);
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
              saveData: !!(nav && nav.connection && nav.connection.saveData),
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
      wasm.init_dense_wasm(lane.id, config, tokenizer, weights);
      state.dense = { lane, kind: "wasm" };
    }

    async function loadWebGpuLane(lane) {
      const runtime = lane.runtime;
      postStatus("loading_model", { lane: laneSummary(lane), file: "transformers.js" });
      const tf = await env.loadTransformers();
      if (state.db && tf.env) {
        // Keep the ONNX files in the same IndexedDB store as the wasm lanes: the
        // Cache API rejects multi-hundred-MB entries on some profiles, and one
        // store means one eviction policy (evictStaleFiles) and one consent check.
        tf.env.useCustomCache = true;
        tf.env.customCache = transformersCache();
      }
      if (env.configureTransformers) env.configureTransformers(tf);
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
        for (let i = 0; i < data.length; i++) {
          if (!Number.isFinite(data[i])) {
            throw new Error(`lane ${lane.id}: model produced a non-finite value at dimension ${i}`);
          }
        }
        return data;
      };
      await embed("warm up");
      state.dense = { lane, kind: "webgpu", embed, dtype };
      await idbMetaPut(webGpuMarker(lane, dtype), Date.now());
    }

    function readyMessage() {
      return {
        type: "ready",
        lanes: laneList(),
        lane: state.dense ? state.dense.lane.id : null,
        runtime: state.dense ? state.dense.kind : null,
        arms: { dense: !!state.dense, sparse: state.sparse, bm25: true },
        degraded: lib.filterDesignDegraded(unique(state.degraded)),
        manifest: manifestSummary(),
        hostSkippedLanes: state.hostSkipped.slice(),
      };
    }

    function postReady() {
      post(readyMessage());
    }

    // -- cache check --------------------------------------------------------

    async function handleCacheCheck(msg, reply) {
      try {
        if (state.phase === "dead") throw fatal("search engine crashed; reload to retry");
        if (msg.indexUrl) state.indexUrl = String(msg.indexUrl);
        if (msg.baseUrl != null) state.baseUrl = String(msg.baseUrl);
        if (msg.version != null) state.version = msg.version ? String(msg.version) : null;
        if (msg.denseRuntime) state.denseRuntime = String(msg.denseRuntime);
        await ensureWasm();
        await ensureIndex();
        await ensureDb();
        await ensureGpu();
        const lane = currentCandidate();
        const cached = lane ? await laneCached(lane) : true;
        reply({
          type: "cache_result",
          requestId: msg.requestId,
          cached,
          lane: lane ? laneSummary(lane) : null,
          sizeBytes: lane ? lib.laneDownloadBytes(lane) : 0,
          hostSkippedLanes: state.hostSkipped.slice(),
          phase: state.phase,
        });
      } catch (err) {
        if (isFatal(err)) state.phase = "dead";
        postError(reply, msg.requestId, describe(err), isFatal(err));
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

    /**
     * Query vector from the WebGPU lane, or nothing (the WASM lane embeds inside
     * search()). An embedding failure degrades the dense arm for this query
     * instead of failing the search; `note` carries the reason.
     */
    async function queryVector(query, mode) {
      if (!state.dense || state.dense.kind !== "webgpu") return { laneId: null, vec: null, note: null };
      if (mode !== "hybrid" && mode !== "dense") return { laneId: null, vec: null, note: null };
      try {
        const vec = await state.dense.embed(query);
        return { laneId: state.dense.lane.id, vec, note: null };
      } catch (err) {
        console.warn(`eddie: dense lane ${state.dense.lane.id} query embedding failed`, err);
        return { laneId: null, vec: null, note: `dense: embedding failed for lane ${state.dense.lane.id}: ${describe(err)}` };
      }
    }

    async function handleSearch(msg, reply) {
      const requestId = msg.requestId;
      try {
        requireIndex();
        const query = String(msg.query || "").trim();
        if (!query) throw new Error("search: empty query");
        const mode = msg.mode || "hybrid";
        const topK = Number(msg.topK) > 0 ? Number(msg.topK) : 8;
        const { laneId, vec, note } = await queryVector(query, mode);
        const res = JSON.parse(wasm.search(query, topK, mode, laneId, vec));
        if (note) res.degraded = (res.degraded || []).concat([note]);
        const results = res.results || [];
        if (msg.evidence) {
          for (const r of results) {
            try {
              r.text = JSON.parse(wasm.chunk(r.chunk)).text;
            } catch (_) {
              r.text = r.snippet;
            }
          }
        }
        let qa = undefined;
        if (msg.qa && Number(msg.qa) > 0 && hasSection("qa") && (vec || !laneId)) {
          qa = JSON.parse(wasm.qa_lookup(query, laneId, vec, Number(msg.qa)));
        }
        reply({
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
        failRequest(reply, requestId, err);
      }
    }

    function handleLookup(msg, reply, type, key, fn) {
      try {
        requireIndex();
        reply({ type, requestId: msg.requestId, [key]: JSON.parse(fn()) });
      } catch (err) {
        failRequest(reply, msg.requestId, err);
      }
      return Promise.resolve();
    }

    async function handleQa(msg, reply) {
      try {
        requireIndex();
        const query = String(msg.query || "").trim();
        const k = Number(msg.k) > 0 ? Number(msg.k) : 3;
        if (!query || !hasSection("qa")) {
          reply({ type: "qa_result", requestId: msg.requestId, hits: [] });
          return;
        }
        const { laneId, vec } = await queryVector(query, "dense");
        const hits = JSON.parse(wasm.qa_lookup(query, laneId, vec, k));
        reply({ type: "qa_result", requestId: msg.requestId, hits });
      } catch (err) {
        failRequest(reply, msg.requestId, err);
      }
    }

    function failRequest(reply, requestId, err) {
      if (isFatal(err)) {
        // A trapped panic leaves the WASM instance unusable; never re-enter it.
        state.phase = "dead";
        postError(reply, requestId, "search engine crashed: " + describe(err), true);
        return;
      }
      postError(reply, requestId, describe(err), false);
    }

    function requireIndex() {
      if (state.phase === "dead") throw fatal("search engine crashed; reload to retry");
      if (!state.indexLoaded) throw new Error(NOT_LOADED);
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
        fetch: env.fetch,
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
      if (!idb) return;
      try {
        state.db = await new Promise((resolve, reject) => {
          const req = idb.open(IDB_NAME, IDB_VERSION);
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

    /**
     * transformers.js cache (`env.customCache`) over the model-file store.
     * transformers.js calls `match`/`put` with string keys: the HuggingFace URL
     * for remote files (mapped onto the repo@revision/file scheme) or a local
     * model path (never stored, so `match` misses and the URL key is tried next).
     */
    function transformersCache() {
      const keyOf = (req) => {
        const url = typeof req === "string" ? req : req && req.url ? req.url : String(req);
        return lib.cacheKeyFromUrl(url) || "url:" + url;
      };
      return {
        async match(req) {
          const stored = await idbGet(keyOf(req));
          if (!stored) return undefined;
          const size = typeof stored.size === "number" ? stored.size : stored.byteLength;
          return new Response(stored, { headers: { "Content-Length": String(size) } });
        },
        async put(req, response) {
          const blob = await response.blob();
          await idbPut(keyOf(req), blob);
        },
      };
    }

    /** Delete cached files that belong to models this index no longer references. */
    async function evictStaleFiles() {
      if (!state.db || !state.manifest) return;
      const keep = ["url:"];
      for (const lane of state.manifest.dense || []) {
        keep.push(`${lib.laneRepo(lane)}@${lib.laneRevision(lane)}/`);
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

    function postStatus(stateName, extra) {
      post(Object.assign({ type: "status", state: stateName }, extra || {}));
    }

    function postError(reply, requestId, message, isFatalErr, isUnsupportedErr) {
      reply({ type: "error", requestId: requestId == null ? undefined : requestId, message, fatal: !!isFatalErr, unsupported: !!isUnsupportedErr });
    }

    return {
      handle,
      state: snapshot,
      readyMessage,
      get phase() {
        return state.phase;
      },
    };
  }

  function unique(list) {
    return Array.from(new Set(list));
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

  /** True for the engine's "not initialised" replies (the client re-runs init). */
  function isNotLoadedMessage(message) {
    return typeof message === "string" && message.indexOf(NOT_LOADED) === 0;
  }

  return { createSearchEngine, NOT_LOADED, isNotLoadedMessage };
});
