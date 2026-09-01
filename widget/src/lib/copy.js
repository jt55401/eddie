// SPDX-License-Identifier: GPL-3.0-only

// User-facing copy shared by the widget and the search engine: byte
// formatting, the download consent card and the degraded-arm notices.
// Split out of lanes.js so the widget bundle carries only this, not the
// engine-side lane selection.

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
    formatBytes,
    consentCopy,
    filterDesignDegraded,
    degradedNotice,
  };
});
