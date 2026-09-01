// SPDX-License-Identifier: GPL-3.0-only

// Agent helpers the widget shows in its UI: model selection, evidence
// assembly, stream display and the FAQ gate. Pure functions; no WebLLM, no
// DOM. The prompts and answer post-processing that run beside the model
// live in agent-llm.js (bundled only into the agent engine's hosts).

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

  const NOHIT = "The site doesn't cover that.";

  const AGENT_MODEL_SIZES = {
    "Qwen3.5-0.8B": 0.4e9,
    "Qwen3.5-2B": 1.2e9,
    "Qwen3.5-4B": 2.3e9,
  };
  const TWO_GIB = 2 * 1024 * 1024 * 1024;

  /** Strip the WebLLM variant suffix to get the family name shown to visitors. */
  function baseModelId(id) {
    return String(id).replace(/-q\d+f(16|32)_\d+-MLC$/i, "");
  }

  function agentModelBytes(id) {
    const base = baseModelId(id);
    return Object.prototype.hasOwnProperty.call(AGENT_MODEL_SIZES, base) ? AGENT_MODEL_SIZES[base] : null;
  }

  /**
   * Choose the WebLLM model id.
   * opts: { mode: "auto"|"light"|"quality"|<id>, maxBufferSize, isMobile, hasF16 }
   *
   * "light" and "quality" are the two sizes the settings panel offers by
   * name; "auto" picks between them from the adapter's buffer limit.
   */
  function selectAgentModel(opts) {
    const o = opts || {};
    const mode = (o.mode || "auto").trim();
    const suffix = o.hasF16 ? "-q4f16_1-MLC" : "-q4f32_1-MLC";
    let base;
    if (mode === "auto") {
      const big = Number(o.maxBufferSize) >= TWO_GIB && !o.isMobile;
      base = big ? "Qwen3.5-2B" : "Qwen3.5-0.8B";
    } else if (mode === "quality") {
      base = "Qwen3.5-2B";
    } else if (mode === "light") {
      base = "Qwen3.5-0.8B";
    } else {
      return { id: mode, base: baseModelId(mode), sizeBytes: agentModelBytes(mode), explicit: true };
    }
    const id = base + suffix;
    return { id, base, sizeBytes: AGENT_MODEL_SIZES[base], explicit: false };
  }

  function isMobileDevice(nav) {
    const n = nav || {};
    if (n.userAgentData && typeof n.userAgentData.mobile === "boolean") {
      return n.userAgentData.mobile;
    }
    return /Mobi|Android|iPhone|iPad|iPod|Windows Phone/i.test(n.userAgent || "");
  }

  /**
   * Display text for a partial stream: complete think blocks removed, and
   * anything after an unclosed <think> hidden until it closes.
   */
  function visibleStreamText(partial) {
    if (!partial) return "";
    let out = String(partial).replace(/<think>[\s\S]*?<\/think>/g, "");
    const open = out.indexOf("<think>");
    if (open >= 0) out = out.substring(0, open);
    return out.replace(/^\s+/, "");
  }

  function urlKey(url) {
    return String(url || "").replace(/#.*$/, "").replace(/\/+$/, "").toLowerCase();
  }

  /**
   * Round-robin merge of several result lists, deduplicated by URL, at most
   * `max` items. Each result keeps its own fields.
   */
  function mergeEvidence(lists, max) {
    const limit = max == null ? 6 : max;
    const seen = new Set();
    const out = [];
    const arrays = (lists || []).map((l) => (Array.isArray(l) ? l : []));
    const longest = arrays.reduce((n, l) => Math.max(n, l.length), 0);
    for (let i = 0; i < longest && out.length < limit; i++) {
      for (const list of arrays) {
        if (out.length >= limit) break;
        const r = list[i];
        if (!r || !r.url) continue;
        const key = urlKey(r.url);
        if (seen.has(key)) continue;
        seen.add(key);
        out.push(r);
      }
    }
    return out;
  }

  // FAQ card gate: prefer the WASM's fused `confident` flag (qa_lookup v0.4.1+);
  // older indexes only carry a dense score, so fall back to a plain cutoff.
  function faqPasses(hit, qaMode) {
    if (!hit || typeof hit !== "object") return false;
    if (qaMode === "off") return false;
    if (qaMode === "always") return true;
    if (typeof hit.confident === "boolean") return hit.confident;
    return typeof hit.score === "number" && hit.score >= 0.5;
  }

  // Turn confident QA hits into agent evidence items ("Q: … A: …") so the
  // answer model sees the FAQ lane, not only chunk text.
  function qaEvidence(hits, max) {
    const limit = max == null ? 2 : max;
    const out = [];
    for (const h of Array.isArray(hits) ? hits : []) {
      if (out.length >= limit) break;
      if (!faqPasses(h, "auto")) continue;
      const q = String(h.question || "").trim();
      const a = String(h.answer || "").trim();
      if (!q || !a) continue;
      out.push({ title: "FAQ: " + q, url: h.source_url || "", text: "Q: " + q + "\nA: " + a, faq: true });
    }
    return out;
  }

  return {
    faqPasses,
    qaEvidence,
    NOHIT,
    AGENT_MODEL_SIZES,
    baseModelId,
    agentModelBytes,
    selectAgentModel,
    isMobileDevice,
    visibleStreamText,
    mergeEvidence,
  };
});
