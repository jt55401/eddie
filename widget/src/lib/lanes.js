// SPDX-License-Identifier: GPL-3.0-only

// Dense-lane selection and download sizes. Pure functions over the index
// manifest (see src/manifest.rs). The consent and notice copy lives in
// copy.js so the widget bundle does not carry the engine-side lane logic.

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

  const DEFAULT_WASM_FILES = ["config.json", "tokenizer.json", "model.safetensors"];

  // Approximate download sizes (bytes) keyed by lower-cased HuggingFace repo.
  const DOWNLOAD_SIZES = {
    "sentence-transformers/multi-qa-minilm-l6-cos-v1": 91e6,
    "sentence-transformers/all-minilm-l6-v2": 91e6,
    "sentence-transformers/all-minilm-l12-v2": 134e6,
    "sentence-transformers/paraphrase-multilingual-minilm-l12-v2": 471e6,
    "baai/bge-small-en-v1.5": 134e6,
    "baai/bge-base-en-v1.5": 438e6,
    "onnx-community/qwen3-embedding-0.6b-onnx": 900e6,
    "onnx-community/harrier-oss-v1-0.6b-onnx": 400e6,
    "xenova/bge-m3": 570e6,
  };

  function laneKind(lane) {
    return lane && lane.runtime ? lane.runtime.kind : null;
  }

  function isWasmLane(lane) {
    return laneKind(lane) === "wasm-candle";
  }

  function isWebGpuLane(lane) {
    return laneKind(lane) === "webgpu-onnx";
  }

  /** Repo the runtime downloads from. */
  function laneRepo(lane) {
    if (isWebGpuLane(lane)) return lane.runtime.repo;
    return lane.model;
  }

  /**
   * HuggingFace revision for the runtime's downloads. `lane.revision` pins
   * `lane.model` (the repo the indexer used); a webgpu-onnx lane downloads
   * from a different repo (`runtime.repo`) that the manifest does not pin.
   */
  function laneRevision(lane) {
    if (isWebGpuLane(lane)) return "main";
    return lane.revision || "main";
  }

  /** Files a wasm-candle lane needs (config, tokenizer, weights). */
  function laneFiles(lane) {
    if (!isWasmLane(lane)) return [];
    const files = Array.isArray(lane.runtime.files) ? lane.runtime.files : [];
    return files.length ? files.slice() : DEFAULT_WASM_FILES.slice();
  }

  function isWeightsFile(file) {
    return /\.(safetensors|bin|pt|gguf|onnx)$/i.test(String(file));
  }

  /** Site-relative directory the lane's model files live in (`eddie index --bundle-model`), or null for HuggingFace. */
  function laneBaseUrl(lane) {
    const b = lane && lane.runtime && lane.runtime.base_url;
    return typeof b === "string" && b.trim() ? b.trim() : null;
  }

  /** Where the lane's files are downloaded from: "site" (bundled next to the index) or "huggingface". */
  function laneOrigin(lane) {
    return laneBaseUrl(lane) ? "site" : "huggingface";
  }

  /** Absolute URL of a site-bundled model file, resolved against the index URL (no version parameter yet). */
  function siteModelUrl(lane, file, indexUrl) {
    const base = laneBaseUrl(lane);
    if (!base) return null;
    return new URL(base.replace(/\/?$/, "/") + String(file).replace(/^\//, ""), indexUrl).href;
  }

  /**
   * Name a lane's file is cached under (see urls.js cacheKey): a bundled f16
   * copy and the HuggingFace original differ, so site files get their own
   * prefix and never collide with a repo download of the same name.
   */
  function laneFileName(lane, file) {
    return laneBaseUrl(lane) ? "@site/" + file : String(file);
  }

  /** The sidecar entry that holds `scope`'s section of `laneId`, or null when the section is in the core file. */
  function sidecarFor(manifest, scope, laneId) {
    const list = manifest && Array.isArray(manifest.sidecars) ? manifest.sidecars : [];
    return list.find((s) => s.scope === scope && s.lane === laneId) || null;
  }

  /** Bytes of the sidecar file(s) a lane's chunk vectors need, counted once per file. */
  function laneSidecarBytes(manifest, laneId) {
    const side = sidecarFor(manifest, "chunks", laneId);
    return side && Number.isFinite(Number(side.bytes)) ? Number(side.bytes) : 0;
  }

  /**
   * Why the WASM loader (init_dense_wasm) cannot run a wasm-candle lane, or
   * null when it can: it accepts BERT-family lanes with config.json,
   * tokenizer.json and exactly one unsharded model.safetensors.
   */
  function wasmLaneProblem(lane) {
    if (!isWasmLane(lane)) return "not a wasm-candle lane";
    if (lane.family && lane.family !== "bert") return `family ${lane.family} is not supported in WASM`;
    const files = laneFiles(lane);
    for (const need of ["config.json", "tokenizer.json"]) {
      if (!files.includes(need)) return `runtime.files lacks ${need}`;
    }
    const weights = files.filter(isWeightsFile);
    if (weights.length === 1 && weights[0] === "model.safetensors") return null;
    if (weights.length === 0) return "runtime.files lists no weights file";
    return `WASM needs a single model.safetensors, not ${weights.join(", ")}`;
  }

  /**
   * Ordered dense-lane candidates for this browser, plus the lanes skipped
   * because this runtime cannot load them (`skipped: [{lane, reason}]`).
   * `denseRuntime`: "auto" | "wasm" | "webgpu"; `hasWebGpu`: an adapter was obtained.
   */
  function chooseDenseLanes(manifest, opts) {
    const lanes = (manifest && Array.isArray(manifest.dense)) ? manifest.dense : [];
    const runtime = (opts && opts.denseRuntime) || "auto";
    const hasWebGpu = !!(opts && opts.hasWebGpu);
    const webgpu = lanes.filter(isWebGpuLane);
    const skipped = [];
    const wasm = lanes.filter(isWasmLane).filter((lane) => {
      const problem = wasmLaneProblem(lane);
      if (problem) skipped.push({ lane, reason: problem });
      return !problem;
    });
    if (lanes.length === 0) {
      return { candidates: [], skipped, reason: "index has no dense lane" };
    }
    let candidates;
    if (runtime === "wasm") {
      candidates = wasm;
    } else if (runtime === "webgpu") {
      candidates = hasWebGpu ? webgpu : [];
    } else {
      candidates = hasWebGpu ? webgpu.concat(wasm) : wasm;
    }
    let reason = null;
    if (candidates.length === 0) {
      if (runtime === "wasm") reason = skipped.length ? "no wasm-candle lane the WASM loader can run" : "index has no wasm-candle lane";
      else if (!hasWebGpu && webgpu.length && !wasm.length) reason = "no WebGPU adapter";
      else if (runtime === "webgpu" && !hasWebGpu) reason = "no WebGPU adapter";
      else if (runtime === "webgpu") reason = "index has no webgpu-onnx lane";
      else reason = "no runnable dense lane";
    }
    return { candidates, skipped, reason };
  }

  /** transformers.js dtype for a webgpu-onnx lane on this adapter. */
  function pickDtype(runtime, hasF16) {
    if (hasF16 && runtime.dtype_f16) return runtime.dtype_f16;
    return runtime.dtype || "fp32";
  }

  /** The exact byte count the manifest declares for a bundled model (`runtime.bytes`), or null. */
  function laneDeclaredBytes(lane) {
    const declared = lane && lane.runtime ? Number(lane.runtime.bytes) : NaN;
    return Number.isFinite(declared) && declared > 0 ? declared : null;
  }

  /**
   * Bytes a lane downloads on first use, or null when unknown: the exact
   * count the manifest carries for a bundled model (`runtime.bytes`, written
   * by `eddie index --bundle-model`), else the table estimate for known
   * HuggingFace repos. The table describes the f32 originals; a bundled f16
   * copy without `runtime.bytes` should be measured instead (the engine
   * HEADs the files), never estimated from the table.
   */
  function laneDownloadBytes(lane) {
    const declared = laneDeclaredBytes(lane);
    if (declared != null) return declared;
    const repo = (laneRepo(lane) || "").toLowerCase();
    return Object.prototype.hasOwnProperty.call(DOWNLOAD_SIZES, repo) ? DOWNLOAD_SIZES[repo] : null;
  }

  return {
    DEFAULT_WASM_FILES,
    DOWNLOAD_SIZES,
    laneKind,
    isWasmLane,
    isWebGpuLane,
    laneRepo,
    laneRevision,
    laneFiles,
    isWeightsFile,
    laneBaseUrl,
    laneOrigin,
    siteModelUrl,
    laneFileName,
    sidecarFor,
    laneSidecarBytes,
    wasmLaneProblem,
    chooseDenseLanes,
    pickDtype,
    laneDeclaredBytes,
    laneDownloadBytes,
  };
});
