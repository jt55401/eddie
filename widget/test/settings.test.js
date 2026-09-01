// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const S = require("../src/lib/settings.js");

function storage(initial) {
  const map = new Map(Object.entries(initial || {}));
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, String(v)),
    removeItem: (k) => map.delete(k),
    _map: map,
  };
}

const LANES = [
  { id: "minilm", model: "sentence-transformers/multi-qa-MiniLM-L6-cos-v1", kind: "wasm-candle", origin: "site" },
  { id: "bge-small", model: "BAAI/bge-small-en-v1.5", kind: "webgpu-onnx", origin: "huggingface" },
];
const SITE = { denseRuntime: "auto", agentMode: "auto", agentModel: "auto", warm: "auto", persist: "auto" };

test("readSettings tolerates missing, malformed and hostile values", () => {
  assert.deepEqual(S.readSettings(storage()), {});
  assert.deepEqual(S.readSettings(storage({ "eddie.settings": "not json" })), {});
  assert.deepEqual(S.readSettings(storage({ "eddie.settings": "[1,2]" })), {});
  assert.deepEqual(S.readSettings(storage({ "eddie.settings": "null" })), {});
  // Only the known string fields survive.
  assert.deepEqual(
    S.readSettings(storage({ "eddie.settings": JSON.stringify({ warm: "off", nope: "x", persist: 7 }) })),
    { warm: "off" }
  );
  // Storage that throws (Safari private mode) reads as "no preferences".
  assert.deepEqual(S.readSettings({ getItem() { throw new Error("denied"); } }), {});
});

test("writeSettings merges, drops empties and survives unusable storage", () => {
  const st = storage();
  assert.deepEqual(S.writeSettings(st, { warm: "off" }), { warm: "off" });
  assert.deepEqual(S.writeSettings(st, { persist: "off" }), { warm: "off", persist: "off" });
  assert.deepEqual(S.writeSettings(st, { warm: null }), { persist: "off" });
  assert.equal(st.getItem(S.SETTINGS_KEY), JSON.stringify({ persist: "off" }));
  assert.deepEqual(S.clearSettings(st), {});
  assert.equal(st.getItem(S.SETTINGS_KEY), null);
  assert.doesNotThrow(() => S.writeSettings({ setItem() { throw new Error("full"); } }, { warm: "off" }));
});

test("choices: every runnable lane, plus keyword-only, which is always offered", () => {
  const c = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(c.search.map((x) => x.value), ["none", "minilm", "bge-small"]);
  assert.equal(c.search[1].detail, "CPU");
  assert.equal(c.search[2].detail, "WebGPU");
});

test("choices: the host's limits remove lanes it cannot run", () => {
  const noGpu = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: false });
  assert.deepEqual(noGpu.search.map((x) => x.value), ["none", "minilm"]);
  const skipped = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: ["minilm"], hasWebGpu: true });
  assert.deepEqual(skipped.search.map((x) => x.value), ["none", "bge-small"]);
});

test("choices: the site's data-* config is a ceiling, never a floor", () => {
  const wasmOnly = S.settingsChoices({ config: { ...SITE, denseRuntime: "wasm" }, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(wasmOnly.search.map((x) => x.value), ["none", "minilm"]);

  const denseOff = S.settingsChoices({ config: { ...SITE, denseRuntime: "off" }, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(denseOff.search.map((x) => x.value), ["none"]);

  const agentOff = S.settingsChoices({ config: { ...SITE, agentMode: "off" }, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(agentOff.agent.map((x) => x.value), ["off"]);

  // Ladders stop at the site's rung.
  const warmOff = S.settingsChoices({ config: { ...SITE, warm: "off" }, lanes: [], hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(warmOff.warm.map((x) => x.value), ["off"]);
  const warmAlways = S.settingsChoices({ config: { ...SITE, warm: "always" }, lanes: [], hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(warmAlways.warm.map((x) => x.value), ["off", "auto", "always"]);
  const persistOff = S.settingsChoices({ config: { ...SITE, persist: "off" }, lanes: [], hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(persistOff.persist.map((x) => x.value), ["off"]);
});

test("choices: the agent needs WebGPU, and a site-pinned model is the only one offered", () => {
  const noGpu = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: false });
  assert.deepEqual(noGpu.agent.map((x) => x.value), ["off"]);

  const auto = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.deepEqual(auto.agent.map((x) => x.value), ["off", "light", "quality"]);

  const pinned = S.settingsChoices({
    config: { ...SITE, agentModel: "Qwen3.5-4B-q4f16_1-MLC" },
    lanes: LANES, hostSkipped: [], hasWebGpu: true,
  });
  assert.deepEqual(pinned.agent.map((x) => x.value), ["off", "Qwen3.5-4B-q4f16_1-MLC"]);
});

test("currentSelection falls back to the running lane and the site defaults", () => {
  const c = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  const none = S.currentSelection(c, {}, { config: SITE, activeLane: "bge-small" });
  assert.equal(none.searchLane, "bge-small");
  assert.equal(none.warm, "auto");
  assert.equal(none.persist, "auto");

  // With the site on agentModel="auto" the panel shows the level auto actually
  // picked for this adapter, not a guess.
  assert.equal(S.currentSelection(c, {}, { config: SITE, activeAgent: "light" }).agentLevel, "light");
  assert.equal(S.currentSelection(c, { agentLevel: "quality" }, { config: SITE, activeAgent: "light" }).agentLevel, "quality");

  const stored = S.currentSelection(c, { searchLane: "minilm", warm: "off" }, { config: SITE, activeLane: "bge-small" });
  assert.equal(stored.searchLane, "minilm");
  assert.equal(stored.warm, "off");

  // A preference for a lane this index no longer has falls back, it does not stick.
  const stale = S.currentSelection(c, { searchLane: "qwen3e" }, { config: SITE, activeLane: "minilm" });
  assert.equal(stale.searchLane, "minilm");

  // Nothing stored and no lane running yet (the panel opened while the engine
  // was still loading): show the least-eager option, never the heaviest.
  const unknown = S.currentSelection(c, {}, { config: SITE, activeLane: null });
  assert.equal(unknown.searchLane, "none");
});

test("effectiveConfig applies only what is on offer", () => {
  const c = S.settingsChoices({ config: SITE, lanes: LANES, hostSkipped: [], hasWebGpu: true });

  const off = S.effectiveConfig(SITE, { searchLane: "none", agentLevel: "off" }, c);
  assert.equal(off.denseRuntime, "off");
  assert.equal(off.laneId, null);
  assert.equal(off.agentMode, "off");

  const pinned = S.effectiveConfig(SITE, { searchLane: "minilm", agentLevel: "light" }, c);
  assert.equal(pinned.laneId, "minilm");
  assert.equal(pinned.denseRuntime, "auto");
  assert.equal(pinned.agentMode, "auto");
  assert.equal(pinned.agentModel, "light");

  // A lane that is not on offer here leaves the site config untouched.
  const stale = S.effectiveConfig(SITE, { searchLane: "qwen3e", warm: "always" }, c);
  assert.equal(stale.laneId, undefined);
  assert.equal(stale.denseRuntime, "auto");
  assert.equal(stale.warm, "auto", "warm=always is above the site's ceiling of auto");

  // Choosing a lane on a site that ships data-dense-runtime="off" cannot happen:
  // the group offers only "none", so the preference is ignored.
  const siteOff = { ...SITE, denseRuntime: "off" };
  const cOff = S.settingsChoices({ config: siteOff, lanes: LANES, hostSkipped: [], hasWebGpu: true });
  assert.equal(S.effectiveConfig(siteOff, { searchLane: "minilm" }, cOff).denseRuntime, "off");
});

test("before the lane list is known, groups are unknown and the ceiling decides", () => {
  const c = S.settingsChoices({ config: SITE, lanes: null });
  assert.equal(c.search, null, "no lane list yet");
  assert.equal(c.agent, null, "no adapter probe yet");
  assert.deepEqual(c.warm.map((x) => x.value), ["off", "auto"]);

  // A stored lane preference survives the mount-time pass; the engine
  // validates it and reports if it cannot run.
  const eff = S.effectiveConfig(SITE, { searchLane: "minilm", agentLevel: "light", warm: "off" }, c);
  assert.equal(eff.laneId, "minilm");
  assert.equal(eff.agentModel, "light");
  assert.equal(eff.warm, "off");

  // The ceiling still applies without a lane list.
  const offSite = { ...SITE, denseRuntime: "off", agentMode: "off" };
  const cOff = S.settingsChoices({ config: offSite, lanes: null });
  const effOff = S.effectiveConfig(offSite, { searchLane: "minilm", agentLevel: "quality" }, cOff);
  assert.equal(effOff.laneId, undefined);
  assert.equal(effOff.denseRuntime, "off");
  assert.equal(effOff.agentMode, "off");

  // Turning search off needs no lane list either.
  assert.equal(S.effectiveConfig(SITE, { searchLane: "none" }, c).denseRuntime, "off");
});
