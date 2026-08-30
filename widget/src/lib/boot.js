// SPDX-License-Identifier: GPL-3.0-only

// Boot loader decisions (widget/src/eddie-boot.js). Pure and tiny: the boot
// script is on every page view, so it carries only what it needs to draw
// the trigger button and decide when to fetch the full widget.

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

  const POSITIONS = ["top-left", "top-right", "bottom-left", "bottom-right"];
  const SEARCH_CONSENT_KEY = "eddie.search.consent";
  // Set on the first modal open on this browser: the boot loader treats the
  // visitor as returning from then on (see decideBoot / decideWarm).
  const SEARCH_USED_KEY = "eddie.search.used";

  /** Trigger placement and theme from the `data-*` attributes (same rules as config.js). */
  function bootLayout(get) {
    const pos = String(get("data-position") || "").trim().toLowerCase();
    const theme = String(get("data-theme") || "").trim().toLowerCase();
    const num = (raw) => {
      const v = Number(raw);
      return raw != null && raw !== "" && Number.isFinite(v) ? Math.trunc(v) : 0;
    };
    const warm = String(get("data-warm") || "").trim().toLowerCase();
    return {
      position: POSITIONS.includes(pos) ? pos : "bottom-right",
      theme: ["auto", "light", "dark"].includes(theme) ? theme : "auto",
      offsetX: num(get("data-offset-x")),
      offsetY: num(get("data-offset-y")),
      warm: ["auto", "off", "always"].includes(warm) ? warm : "auto",
    };
  }

  /**
   * When the boot loader fetches the full widget.
   *   warm        "auto" | "off" | "always" (data-warm)
   *   saveData    navigator.connection.saveData
   *   reducedData prefers-reduced-data: reduce
   *   used        the visitor opened the search before on this browser
   *   consented   a dense lane was accepted before on this browser
   * Returns { action: "idle" | "interaction", reason }: "idle" loads the
   * widget after the page has loaded and the browser is idle (the widget then
   * runs its own warm-up), "interaction" waits for the trigger, the shortcut
   * or a programmatic open.
   */
  function decideBoot(opts) {
    const o = opts || {};
    if (o.warm === "off") return { action: "interaction", reason: "warm is off" };
    if (o.saveData) return { action: "interaction", reason: "data saver" };
    if (o.reducedData) return { action: "interaction", reason: "prefers-reduced-data" };
    if (o.warm === "always") return { action: "idle", reason: "warm is always" };
    if (o.consented) return { action: "idle", reason: "model consented before" };
    if (o.used) return { action: "idle", reason: "search used before" };
    return { action: "interaction", reason: "first visit" };
  }

  /** `v` query parameter of a URL string, or null. */
  function bootVersionOf(href) {
    const m = /[?&]v=([^&#]+)/.exec(String(href || ""));
    return m ? decodeURIComponent(m[1]) : null;
  }

  /**
   * URL of the full widget next to the boot script, carrying the boot
   * script's `?v=` (or the index URL's when the script has none).
   */
  function widgetScriptUrl(scriptHref, indexUrl) {
    const base = scriptHref.split(/[?#]/)[0];
    const dir = base.substring(0, base.lastIndexOf("/") + 1);
    const v = bootVersionOf(scriptHref) || bootVersionOf(indexUrl);
    return dir + "eddie-widget.js" + (v ? "?v=" + encodeURIComponent(v) : "");
  }

  return { SEARCH_CONSENT_KEY, SEARCH_USED_KEY, bootLayout, decideBoot, bootVersionOf, widgetScriptUrl };
});
