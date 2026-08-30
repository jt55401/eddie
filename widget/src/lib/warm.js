// SPDX-License-Identifier: GPL-3.0-only

// Warm-at-load decision (`data-warm`): whether the widget should initialise
// the search engine before the visitor opens the modal. Pure; the widget
// supplies what it knows and acts on the returned action.

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

  const SEARCH_CONSENT_KEY = "eddie.search.consent";

  /**
   * opts:
   *   mode          "auto" | "off" | "always"
   *   saveData      navigator.connection.saveData
   *   engineReady   a persistent engine already reports `ready` for this index
   *   checked       a cache_check result is in (lane/cached below are valid)
   *   lane          the dense lane the engine would load ({id} or null)
   *   cached        that lane's files are in IndexedDB
   *   consentedLane lane id from localStorage (the visitor accepted it before)
   *
   * Returns { action: "none" | "adopt" | "check" | "init", consent, reason }.
   *   adopt: take over the ready engine, no init needed
   *   check: run cache_check (site assets only), then call again with the result
   *   init : send init; `consent` says whether to pass consent=true
   */
  function decideWarm(opts) {
    const o = opts || {};
    const mode = o.mode || "auto";
    if (mode === "off") return { action: "none", consent: false, reason: "warm is off" };
    if (o.engineReady) return { action: "adopt", consent: false, reason: "engine already ready" };
    if (!o.checked) {
      if (mode === "auto" && o.saveData) return { action: "none", consent: false, reason: "data saver" };
      return { action: "check", consent: false, reason: "cache state unknown" };
    }
    if (!o.lane) return { action: "init", consent: false, reason: "no dense lane to download" };
    if (o.cached) {
      if (mode === "always") return { action: "init", consent: false, reason: "lane cached (always)" };
      if (o.consentedLane && o.consentedLane === o.lane.id) return { action: "init", consent: false, reason: "lane cached and consented before" };
      return { action: "none", consent: false, reason: "lane cached but never consented on this browser" };
    }
    if (mode === "always") {
      if (o.saveData) return { action: "none", consent: false, reason: "data saver" };
      return { action: "init", consent: true, reason: "always warms uncached lanes" };
    }
    return { action: "none", consent: false, reason: "lane not cached" };
  }

  return { SEARCH_CONSENT_KEY, decideWarm };
});
