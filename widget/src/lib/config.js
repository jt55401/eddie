// SPDX-License-Identifier: GPL-3.0-only

// Widget configuration: parse the data-* attributes of the <script> tag.
// Pure; `get(name)` returns the attribute string or null.

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

  const POSITIONS = new Set(["top-left", "top-right", "bottom-left", "bottom-right"]);

  function oneOf(raw, allowed, fallback) {
    const value = (raw || "").trim().toLowerCase();
    return allowed.includes(value) ? value : fallback;
  }

  function intAttr(raw, fallback) {
    if (raw == null || raw === "") return fallback;
    const value = Number.parseInt(raw, 10);
    return Number.isFinite(value) && value > 0 ? value : fallback;
  }

  function offsetAttr(raw) {
    if (raw == null || raw === "") return 0;
    const value = Number(raw);
    return Number.isFinite(value) ? Math.trunc(value) : 0;
  }

  /**
   * Parse the widget configuration from a `get(attributeName)` accessor.
   * Every value is validated; unknown values fall back to the documented default.
   */
  function parseWidgetConfig(get) {
    const agentModelRaw = (get("data-agent-model") || "").trim();
    return {
      indexUrl: (get("data-index-url") || "").trim(),
      position: (function () {
        const v = (get("data-position") || "").trim().toLowerCase();
        return POSITIONS.has(v) ? v : "bottom-right";
      })(),
      theme: oneOf(get("data-theme"), ["auto", "light", "dark"], "auto"),
      offsetX: offsetAttr(get("data-offset-x")),
      offsetY: offsetAttr(get("data-offset-y")),
      qaMode: oneOf(get("data-qa-mode"), ["off", "auto", "always"], "auto"),
      qaSubject: (get("data-qa-subject") || "").trim(),
      topK: intAttr(get("data-top-k"), 8),
      answerTopK: intAttr(get("data-answer-top-k"), 5),
      agentMode: oneOf(get("data-agent-mode"), ["off", "auto"], "auto"),
      agentModel: agentModelRaw === "" ? "auto" : agentModelRaw,
      denseRuntime: oneOf(get("data-dense-runtime"), ["auto", "wasm", "webgpu", "off"], "auto"),
      consentText: (get("data-consent-text") || "").trim(),
      persist: oneOf(get("data-persist"), ["auto", "off"], "auto"),
      warm: oneOf(get("data-warm"), ["auto", "off", "always"], "auto"),
    };
  }

  return { parseWidgetConfig };
});
