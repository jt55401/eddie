// SPDX-License-Identifier: GPL-3.0-only

// Agent helpers shared by the widget and the agent worker: model selection,
// prompts, plan parsing, evidence assembly and answer post-processing.
// Pure functions; no WebLLM, no DOM.

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
  const NOHIT_RE = /\bthe site (?:doesn['’]t|does not|didn['’]t|did not) cover (?:that|this|it)\.?/gi;

  const AGENT_MODEL_SIZES = {
    "Qwen3.5-0.8B": 0.4e9,
    "Qwen3.5-2B": 1.2e9,
    "Qwen3.5-4B": 2.3e9,
  };
  const TWO_GIB = 2 * 1024 * 1024 * 1024;

  const PLAN_SCHEMA = {
    type: "object",
    properties: {
      queries: { type: "array", items: { type: "string" }, minItems: 1, maxItems: 3 },
    },
    required: ["queries"],
  };

  function planPrompt(site) {
    return `You write search queries for a site search engine. The site is ${site}. Reply with JSON only: {"queries": ["..."]}. Give 1 to 3 different short keyword queries (2 to 5 words each, no punctuation) that a site search engine would match against page text. Each query must be different. Do not answer the question.`;
  }

  function answerPrompt(site) {
    return `You answer visitor questions about ${site} using only the numbered sources below the question. Answer the question directly in the first sentence. Write 1 to 3 sentences in your own words; never repeat a source's wording. End each sentence with the numbers of the sources it comes from, like [2] or [1][3]. Never cite a number that is not in the list. Do not add calculations or inferences that are not in the sources. If no source answers the question, your entire reply is: ${NOHIT} Never mix that sentence with an answer. Never use outside knowledge.`;
  }

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
   * opts: { mode: "auto"|"quality"|<id>, maxBufferSize, isMobile, hasF16 }
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

  /** Remove <think>…</think> blocks; a dangling <think> loses only the tag. */
  function stripThink(text) {
    if (!text) return "";
    let out = String(text).replace(/<think>[\s\S]*?<\/think>/g, "");
    out = out.replace(/<think>/g, "").replace(/<\/think>/g, "");
    return out.trim();
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

  function extractJsonObject(text) {
    const s = String(text);
    const start = s.indexOf("{");
    const end = s.lastIndexOf("}");
    if (start < 0 || end <= start) return null;
    try {
      return JSON.parse(s.substring(start, end + 1));
    } catch (_) {
      return null;
    }
  }

  function cleanQuery(q) {
    return String(q)
      .replace(/\/?no_think/gi, "")
      .replace(/[\s"'`*]+$/g, "")
      .replace(/^[\s"'`*\-\d.)]+/g, "")
      .replace(/\s+/g, " ")
      .trim();
  }

  /** Parse the planner reply into 1..3 distinct queries; falls back to the question. */
  function parsePlan(text, question) {
    const cleaned = stripThink(text);
    const obj = extractJsonObject(cleaned);
    const raw = obj && Array.isArray(obj.queries) ? obj.queries : [];
    const seen = new Set();
    const out = [];
    for (const item of raw) {
      if (typeof item !== "string") continue;
      const q = cleanQuery(item);
      if (q.length < 2 || q.length > 80) continue;
      const key = q.toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(q);
      if (out.length === 3) break;
    }
    if (out.length === 0 && question && question.trim()) {
      out.push(question.trim());
    }
    return out;
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

  /** Cut to `max` characters at a word boundary, with an ellipsis. */
  function truncateText(text, max) {
    const limit = max == null ? 700 : max;
    const s = String(text || "").replace(/\s+/g, " ").trim();
    if (s.length <= limit) return s;
    const cut = s.lastIndexOf(" ", limit - 1);
    return s.substring(0, cut > limit * 0.6 ? cut : limit - 1).trim() + "…";
  }

  /** `[n] title (url)\ntext` blocks joined by blank lines. */
  function formatEvidence(items, maxChars) {
    return (items || [])
      .map((e, i) => `[${i + 1}] ${e.title || e.url} (${e.url})\n${truncateText(e.text || e.snippet || "", maxChars)}`)
      .join("\n\n");
  }

  function sourcesPrompt(items, question, maxChars) {
    return `Sources:\n\n${formatEvidence(items, maxChars)}\n\nQuestion: ${question}`;
  }

  /**
   * Post-process a raw model answer against the evidence list.
   * Returns { answer, citations: [{n, url, title}], nohit }.
   */
  function postProcessAnswer(raw, evidence) {
    const ev = Array.isArray(evidence) ? evidence : [];
    let text = stripThink(raw);
    // **[1]** -> [1]
    text = text.replace(/\*\*\s*((?:\[\s*\d+(?:\s*,\s*\d+)*\s*\]\s*)+)\*\*/g, "$1");
    // Lines that are only citation markers belong to the previous line.
    const lines = text.split(/\r?\n/);
    const merged = [];
    for (const line of lines) {
      const t = line.trim();
      if (t && /^(?:\[\s*\d+(?:\s*,\s*\d+)*\s*\]\s*)+$/.test(t)) {
        let j = merged.length - 1;
        while (j >= 0 && merged[j].trim() === "") j--;
        if (j >= 0) {
          // Append only markers the previous line does not already carry.
          const prev = merged[j];
          const fresh = (t.match(/\[\s*\d+(?:\s*,\s*\d+)*\s*\]/g) || []).filter((m) => !prev.includes(m.replace(/\s+/g, "")));
          merged.length = j + 1;
          if (fresh.length) merged[j] = prev.replace(/\s+$/, "") + " " + fresh.join("");
          continue;
        }
      }
      merged.push(line);
    }
    text = merged.join("\n");

    // Drop the fallback sentence when anything else remains.
    const withoutFallback = text.replace(NOHIT_RE, "").replace(/[ \t]+\n/g, "\n").trim();
    const residue = withoutFallback
      .replace(/\[\s*\d+(?:\s*,\s*\d+)*\s*\]/g, "")
      .replace(/^\s*(?:yes|no)\b/i, "")
      .replace(/[\s.,;:!?'"*-]+/g, "");
    const onlyFallback = residue === "";
    if (onlyFallback) {
      return { answer: NOHIT, citations: [], nohit: true };
    }
    text = withoutFallback;

    // Map [n] citations to evidence; drop out-of-range ones.
    const cited = [];
    const seen = new Set();
    text = text.replace(/\[\s*(\d+(?:\s*,\s*\d+)*)\s*\]/g, (_, body) => {
      const nums = body.split(",").map((x) => Number(x.trim()));
      let out = "";
      for (const n of nums) {
        if (n >= 1 && n <= ev.length) {
          out += `[${n}]`;
          if (!seen.has(n)) {
            seen.add(n);
            cited.push(n);
          }
        }
      }
      return out;
    });
    // Collapse duplicate adjacent markers ("[1][1]") and tidy whitespace.
    text = text.replace(/(\[\d+\])(?:\s*\1)+/g, "$1");
    text = text.replace(/[ \t]{2,}/g, " ").replace(/ +([.,;:!?])/g, "$1").replace(/\n{3,}/g, "\n\n").trim();
    const citations = cited
      .sort((a, b) => a - b)
      .map((n) => ({ n, url: ev[n - 1].url, title: ev[n - 1].title || ev[n - 1].url }));
    return { answer: text, citations, nohit: text === "" };
  }

  return {
    NOHIT,
    AGENT_MODEL_SIZES,
    PLAN_SCHEMA,
    planPrompt,
    answerPrompt,
    sourcesPrompt,
    baseModelId,
    agentModelBytes,
    selectAgentModel,
    isMobileDevice,
    stripThink,
    visibleStreamText,
    parsePlan,
    mergeEvidence,
    truncateText,
    formatEvidence,
    postProcessAnswer,
  };
});
