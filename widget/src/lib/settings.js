// SPDX-License-Identifier: GPL-3.0-only

// Visitor preferences: which search model to download, which agent model to
// run, and how eagerly to load either. Pure functions over the site's
// `data-*` config, the index's lane list and what the browser can run.
//
// The site config is the ceiling: a visitor can always choose less, and can
// choose among what the site left open, but cannot switch on what the owner
// turned off. `settingsChoices` is where that rule lives. `effectiveConfig`
// drops a preference that is no longer on offer -- a re-indexed site, a
// different browser, a changed `data-*` -- so a stale one degrades to the
// site default instead of breaking the widget.

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

  const SETTINGS_KEY = "eddie.settings";

  // Ladders, least eager first: a visitor may pick any rung up to and
  // including the site's.
  const WARM_RUNGS = ["off", "auto", "always"];
  const PERSIST_RUNGS = ["off", "auto"];
  // The two named agent sizes selectAgentModel understands.
  const AGENT_LEVELS = ["light", "quality"];

  const FIELDS = ["searchLane", "agentLevel", "warm", "persist"];

  /** Stored preferences, or {} when there are none, the value is unusable, or storage is unavailable. */
  function readSettings(storage) {
    let raw = null;
    try {
      raw = storage && storage.getItem ? storage.getItem(SETTINGS_KEY) : null;
    } catch (_) {
      return {};
    }
    if (!raw) return {};
    let parsed;
    try {
      parsed = JSON.parse(raw);
    } catch (_) {
      return {};
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out = {};
    for (const f of FIELDS) {
      if (typeof parsed[f] === "string" && parsed[f]) out[f] = parsed[f];
    }
    return out;
  }

  /** Merge `patch` into the stored preferences and persist. Returns the merged object. */
  function writeSettings(storage, patch) {
    const merged = Object.assign(readSettings(storage), patch || {});
    for (const k of Object.keys(merged)) {
      if (merged[k] == null || merged[k] === "") delete merged[k];
    }
    try {
      if (storage && storage.setItem) storage.setItem(SETTINGS_KEY, JSON.stringify(merged));
    } catch (_) {
      // storage unavailable: the choice applies to this page and is forgotten
    }
    return merged;
  }

  function clearSettings(storage) {
    try {
      if (storage && storage.removeItem) storage.removeItem(SETTINGS_KEY);
    } catch (_) {
      // nothing to do
    }
    return {};
  }

  /** Rungs of a ladder up to and including the site's value (which is the ceiling). */
  function rungsUpTo(rungs, siteValue, fallback) {
    const at = rungs.indexOf(siteValue);
    return rungs.slice(0, (at < 0 ? rungs.indexOf(fallback) : at) + 1);
  }

  /** A lane the site's `data-dense-runtime` allows (`auto` allows both kinds). */
  function laneAllowedByRuntime(lane, denseRuntime) {
    if (denseRuntime === "off") return false;
    if (denseRuntime === "wasm") return lane.kind === "wasm-candle";
    if (denseRuntime === "webgpu") return lane.kind === "webgpu-onnx";
    return true;
  }

  /**
   * What the settings panel may offer.
   *
   * o: { config, lanes, hostSkipped, hasWebGpu }, where `lanes` is null
   * before the engine has read the index.
   *
   * `search` and `agent` are null while `lanes` is: at mount there is neither
   * a lane list nor an adapter probe, and answering "no options" then would
   * throw away a valid stored preference. `warm` and `persist` need only the
   * site config. Every non-null group is a list of `{ value, label, detail }`
   * containing the least-eager option, so the panel renders a radio group
   * without special cases.
   */
  function settingsChoices(o) {
    const opts = o || {};
    const config = opts.config || {};
    const lanesKnown = Array.isArray(opts.lanes);
    const lanes = lanesKnown ? opts.lanes : [];
    const skipped = new Set(Array.isArray(opts.hostSkipped) ? opts.hostSkipped : []);
    const hasWebGpu = !!opts.hasWebGpu;

    const search = [{ value: "none", label: "Keyword only", detail: "No model download" }];
    for (const lane of lanes) {
      if (!lane || !lane.id || skipped.has(lane.id)) continue;
      if (!laneAllowedByRuntime(lane, config.denseRuntime || "auto")) continue;
      if (lane.kind === "webgpu-onnx" && !hasWebGpu) continue;
      search.push({
        value: lane.id,
        label: lane.model || lane.id,
        detail: lane.kind === "webgpu-onnx" ? "WebGPU" : "CPU",
        origin: lane.origin || null,
        kind: lane.kind || null,
      });
    }

    // WebLLM needs a WebGPU adapter, so without one the agent is not on offer
    // however the site is configured.
    const agent = [{ value: "off", label: "Off", detail: "No agent download" }];
    const pinned = String(config.agentModel || "auto").trim();
    if (config.agentMode !== "off" && hasWebGpu) {
      if (pinned && pinned !== "auto" && AGENT_LEVELS.indexOf(pinned) < 0) {
        // The site pinned one WebLLM model id: offer that, or nothing.
        agent.push({ value: pinned, label: pinned, detail: "Chosen by this site" });
      } else {
        agent.push({ value: "light", label: "Light", detail: "Smaller model, faster, less accurate" });
        agent.push({ value: "quality", label: "Quality", detail: "Larger model, slower, better answers" });
      }
    }

    const warm = rungsUpTo(WARM_RUNGS, config.warm, "auto").map((value) => ({
      value,
      label: value === "off" ? "On demand" : value === "auto" ? "When I've searched before" : "Always",
      detail:
        value === "off"
          ? "Load search when I first search"
          : value === "auto"
            ? "Preload on pages I visit after my first search here"
            : "Preload on every page view",
    }));

    const persist = rungsUpTo(PERSIST_RUNGS, config.persist, "auto").map((value) => ({
      value,
      label: value === "off" ? "Per page" : "Across pages",
      detail:
        value === "off"
          ? "Reload the engine on every page"
          : "Keep the engine in a service worker so other pages start instantly",
    }));

    return { search: lanesKnown ? search : null, agent: lanesKnown ? agent : null, warm, persist };
  }

  /** Would the site config allow this search preference, lanes aside? */
  function searchWithinCeiling(config, value) {
    if (!value) return false;
    if (value === "none") return true;
    return ((config || {}).denseRuntime || "auto") !== "off";
  }

  /** Would the site config allow this agent preference, adapter aside? */
  function agentWithinCeiling(config, value) {
    if (!value) return false;
    if (value === "off") return true;
    return (config || {}).agentMode !== "off";
  }

  /**
   * What a group shows: the stored preference if it is still on offer, else
   * the site default, else the least-eager option -- never the heaviest, so a
   * panel opened before the engine reports a lane does not imply the biggest
   * model is already running.
   */
  function selected(group, stored, fallback) {
    const has = (v) => group.some((c) => c.value === v);
    if (stored && has(stored)) return stored;
    if (fallback && has(fallback)) return fallback;
    return group.length ? group[0].value : null;
  }

  /**
   * Which option each group is on. `searchLane` and `agentLevel` have no site
   * default, so the lane the engine actually loaded stands in for the first
   * and the site's `agentModel` for the second.
   */
  function currentSelection(choices, settings, o) {
    const c = choices || {};
    const s = settings || {};
    const opts = o || {};
    const config = opts.config || {};
    // With the site on `auto` the level is whatever selectAgentModel picked
    // for this adapter, so show that rather than guessing "quality".
    const agentFallback = (function () {
      const pinned = String(config.agentModel || "auto").trim();
      if (pinned && pinned !== "auto") return pinned;
      return opts.activeAgent || "quality";
    })();
    return {
      searchLane: selected(c.search || [], s.searchLane, opts.activeLane || null),
      agentLevel: selected(c.agent || [], s.agentLevel, agentFallback),
      warm: selected(c.warm || [], s.warm, config.warm),
      persist: selected(c.persist || [], s.persist, config.persist),
    };
  }

  /**
   * The site config with the visitor's preferences applied. Preferences that
   * are no longer on offer are ignored, so the result is always something the
   * engine and the transport can act on.
   */
  function effectiveConfig(config, settings, choices) {
    const base = Object.assign({}, config || {});
    const c = choices || {};
    const s = settings || {};
    // A group the widget does not know yet (no lane list at mount) falls back
    // to the site ceiling: the engine validates a pinned lane anyway and
    // reports it if the lane cannot run here.
    const allowed = (group, v, ceiling) =>
      !!v && (group ? group.some((x) => x.value === v) : ceiling(config, v));

    if (allowed(c.search, s.searchLane, searchWithinCeiling)) {
      if (s.searchLane === "none") {
        base.denseRuntime = "off";
        base.laneId = null;
      } else {
        base.laneId = s.searchLane;
        if (base.denseRuntime === "off") base.denseRuntime = "auto";
      }
    }
    if (allowed(c.agent, s.agentLevel, agentWithinCeiling)) {
      if (s.agentLevel === "off") {
        base.agentMode = "off";
      } else {
        base.agentMode = "auto";
        base.agentModel = s.agentLevel;
      }
    }
    if (allowed(c.warm, s.warm, () => false)) base.warm = s.warm;
    if (allowed(c.persist, s.persist, () => false)) base.persist = s.persist;
    return base;
  }

  return {
    SETTINGS_KEY,
    readSettings,
    writeSettings,
    clearSettings,
    settingsChoices,
    currentSelection,
    effectiveConfig,
  };
});
