// SPDX-License-Identifier: GPL-3.0-only

// Dense-lane selection, download sizes, consent copy and degraded-arm notices.
// Pure functions over the index manifest (see src/manifest.rs).

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

  /** Approximate bytes a lane downloads on first use, or null when unknown. */
  function laneDownloadBytes(lane) {
    const repo = (laneRepo(lane) || "").toLowerCase();
    return Object.prototype.hasOwnProperty.call(DOWNLOAD_SIZES, repo) ? DOWNLOAD_SIZES[repo] : null;
  }

  function formatBytes(n) {
    if (n == null || !Number.isFinite(n)) return "unknown size";
    if (n >= 1e9) {
      const g = n / 1e9;
      return (g >= 10 ? Math.round(g) : Math.round(g * 10) / 10) + " GB";
    }
    if (n >= 1e6) return Math.round(n / 1e6) + " MB";
    if (n >= 1e3) return Math.round(n / 1e3) + " KB";
    return n + " B";
  }

  /**
   * Consent card copy for a model download. `consentText` (site override) may
   * contain `{size}`, `{model}` and `{origin}` placeholders.
   *   sizeBytes    model download (null when unknown)
   *   origin       "site" | "huggingface" (where the model is fetched from)
   *   sidecarBytes index vectors fetched from the site for this lane (0 when in the core index)
   */
  function consentCopy(opts) {
    const size = formatBytes(opts.sizeBytes);
    const model = opts.model || "the search model";
    const origin = opts.origin === "site" ? "this site" : "huggingface.co";
    if (opts.consentText) {
      return opts.consentText.replace(/\{size\}/g, size).replace(/\{model\}/g, model).replace(/\{origin\}/g, origin);
    }
    const sized = opts.sizeBytes == null ? `a one-time download (size unknown) from ${origin}` : `a one-time ${size} download from ${origin}`;
    const side = opts.sidecarBytes > 0 ? `, plus ${formatBytes(opts.sidecarBytes)} of index vectors from this site` : "";
    let text = `Semantic search runs ${model} in your browser. That needs ${sized}${side}, kept in your browser's cache for next time.`;
    if (opts.saveData) {
      text = "Data saver is on. " + text;
    }
    return text;
  }

  /**
   * Drop degraded notes that describe the index design rather than a failure
   * (an index without a dense lane or sparse arm is not "degraded").
   */
  function filterDesignDegraded(degraded) {
    return (degraded || []).filter((d) => !/index has no (dense lane|sparse arm)/.test(d));
  }

  /** Human notice when a semantic arm is missing, or null when nothing to say. */
  function degradedNotice(arms, degraded) {
    const named = filterDesignDegraded(degraded).filter((d) => /^(dense|sparse)\b/.test(d));
    if (named.length === 0) return null;
    const a = arms || {};
    if (!a.dense && !a.sparse) {
      return "Keyword-only results: the semantic model isn't available.";
    }
    if (!a.dense) {
      return "Keyword and sparse results only: the dense model isn't available.";
    }
    return "Results without the learned-sparse arm.";
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
    laneDownloadBytes,
    formatBytes,
    consentCopy,
    filterDesignDegraded,
    degradedNotice,
  };
});
