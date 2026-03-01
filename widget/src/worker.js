// SPDX-License-Identifier: GPL-3.0-only

// static-agent Web Worker
//
// Loads WASM module, downloads and caches the ML model in IndexedDB,
// and handles search queries from the main thread.

"use strict";

// -- Configuration --
const HF_CDN = "https://huggingface.co";
const MODEL_FILES = ["config.json", "tokenizer.json", "model.safetensors"];
const IDB_NAME = "static-agent-models";
const IDB_STORE = "files";

// -- State --
let baseUrl = "";
let initialized = false;

// -- Message handler --
self.onmessage = async function (e) {
  const msg = e.data;

  if (msg.type === "init") {
    try {
      await initialize(msg.indexUrl, msg.baseUrl);
    } catch (err) {
      postStatus("error", { error: err.message || String(err) });
    }
  } else if (msg.type === "search") {
    try {
      if (!initialized) {
        throw new Error("Engine not initialized");
      }
      const results = wasm_bindgen.search_query(
        msg.query,
        msg.topK || 5,
        msg.mode || "hybrid"
      );
      self.postMessage({
        type: "search_result",
        requestId: msg.requestId,
        results: results,
      });
    } catch (err) {
      self.postMessage({
        type: "error",
        requestId: msg.requestId,
        error: err.message || String(err),
      });
    }
  }
};

async function initialize(indexUrl, workerBaseUrl) {
  baseUrl = workerBaseUrl || "";

  // 1. Load WASM glue + instantiate
  postStatus("loading_wasm");
  const wasmGlueUrl = resolveUrl("static-agent-wasm.js");
  importScripts(wasmGlueUrl);
  const wasmBinaryUrl = resolveUrl("static-agent.wasm");
  await wasm_bindgen(wasmBinaryUrl);

  // 2. Fetch index
  postStatus("loading_index");
  const indexResponse = await fetch(indexUrl);
  if (!indexResponse.ok) {
    throw new Error(`Failed to fetch index: ${indexResponse.status}`);
  }
  const indexBytes = new Uint8Array(await indexResponse.arrayBuffer());

  // 3. Parse model ID from index header
  const modelId = parseModelIdFromIndex(indexBytes);

  // 4. Fetch model files (with IndexedDB cache)
  postStatus("checking_cache");
  const db = await openModelDB();

  const config = await getCachedOrFetch(
    db,
    modelId,
    "config.json",
    (loaded, total) => postStatus("downloading_model", { progress: loaded / total, file: "config.json" })
  );
  const tokenizer = await getCachedOrFetch(
    db,
    modelId,
    "tokenizer.json",
    (loaded, total) => postStatus("downloading_model", { progress: loaded / total, file: "tokenizer.json" })
  );
  const weights = await getCachedOrFetch(
    db,
    modelId,
    "model.safetensors",
    (loaded, total) => postStatus("downloading_model", { progress: loaded / total, file: "model.safetensors" })
  );

  // 5. Initialize WASM engine
  postStatus("initializing");
  wasm_bindgen.init_engine(
    new Uint8Array(config),
    new Uint8Array(tokenizer),
    new Uint8Array(weights),
    indexBytes
  );

  initialized = true;
  postStatus("ready");
}

// -- Index header parsing --
// Binary format: SAGI (4) + version (4) + dim (4) + num_chunks (4) +
//   model_id_len (4) + metadata_len (4) + model_id bytes
function parseModelIdFromIndex(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const magic = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]);
  if (magic !== "SAGI") {
    throw new Error("Invalid index file (bad magic)");
  }
  // Skip: version(4) + dim(4) + num_chunks(4)
  const modelIdLen = view.getUint32(16, true);
  // Skip: metadata_len(4)
  const modelIdStart = 24; // 4+4+4+4+4+4
  const decoder = new TextDecoder();
  return decoder.decode(bytes.slice(modelIdStart, modelIdStart + modelIdLen));
}

// -- IndexedDB helpers --
function openModelDB() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(IDB_NAME, 1);
    req.onupgradeneeded = () => {
      req.result.createObjectStore(IDB_STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function idbGet(db, key) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readonly");
    const req = tx.objectStore(IDB_STORE).get(key);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function idbPut(db, key, value) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readwrite");
    const req = tx.objectStore(IDB_STORE).put(value, key);
    req.onsuccess = () => resolve();
    req.onerror = () => reject(req.error);
  });
}

async function getCachedOrFetch(db, modelId, filename, onProgress) {
  const key = `${modelId}/${filename}`;
  const cached = await idbGet(db, key);
  if (cached) {
    return cached;
  }

  const url = `${HF_CDN}/${modelId}/resolve/main/${filename}`;
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to download ${filename}: ${response.status}`);
  }

  const contentLength = parseInt(response.headers.get("Content-Length") || "0", 10);
  const reader = response.body.getReader();
  const chunks = [];
  let loaded = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    loaded += value.length;
    if (contentLength > 0) {
      onProgress(loaded, contentLength);
    }
  }

  // Concatenate chunks into single ArrayBuffer
  const total = chunks.reduce((sum, c) => sum + c.length, 0);
  const result = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }

  const buffer = result.buffer;
  await idbPut(db, key, buffer);
  return buffer;
}

// -- Utilities --
function resolveUrl(filename) {
  if (baseUrl) {
    return baseUrl.replace(/\/$/, "") + "/" + filename;
  }
  return filename;
}

function postStatus(state, extra) {
  self.postMessage({ type: "status", state, ...extra });
}
