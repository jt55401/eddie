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

  function laneRevision(lane) {
    return lane.revision || "main";
  }

  /** Files a wasm-candle lane needs (config, tokenizer, weights). */
  function laneFiles(lane) {
    if (!isWasmLane(lane)) return [];
    const files = Array.isArray(lane.runtime.files) ? lane.runtime.files : [];
    return files.length ? files.slice() : DEFAULT_WASM_FILES.slice();
  }

  /**
   * Ordered dense-lane candidates for this browser.
   * `denseRuntime`: "auto" | "wasm" | "webgpu"; `hasWebGpu`: an adapter was obtained.
   */
  function chooseDenseLanes(manifest, opts) {
    const lanes = (manifest && Array.isArray(manifest.dense)) ? manifest.dense : [];
    const runtime = (opts && opts.denseRuntime) || "auto";
    const hasWebGpu = !!(opts && opts.hasWebGpu);
    const webgpu = lanes.filter(isWebGpuLane);
    const wasm = lanes.filter(isWasmLane);
    if (lanes.length === 0) {
      return { candidates: [], reason: "index has no dense lane" };
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
      if (runtime === "wasm") reason = "index has no wasm-candle lane";
      else if (!hasWebGpu && webgpu.length && !wasm.length) reason = "no WebGPU adapter";
      else if (runtime === "webgpu" && !hasWebGpu) reason = "no WebGPU adapter";
      else if (runtime === "webgpu") reason = "index has no webgpu-onnx lane";
      else reason = "no runnable dense lane";
    }
    return { candidates, reason };
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
   * contain `{size}` and `{model}` placeholders.
   */
  function consentCopy(opts) {
    const size = formatBytes(opts.sizeBytes);
    const model = opts.model || "the search model";
    if (opts.consentText) {
      return opts.consentText.replace(/\{size\}/g, size).replace(/\{model\}/g, model);
    }
    const sized = opts.sizeBytes == null ? "a one-time download (size unknown)" : `a one-time ${size} download`;
    let text = `Semantic search runs a language model in your browser. That needs ${sized} of ${model}, kept in your browser's cache for next time.`;
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
    chooseDenseLanes,
    pickDtype,
    laneDownloadBytes,
    formatBytes,
    consentCopy,
    filterDesignDegraded,
    degradedNotice,
  };
});
