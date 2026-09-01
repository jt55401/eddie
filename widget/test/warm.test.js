// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const W = require("../src/lib/warm.js");

const lane = { id: "qwen3e" };

test("warm off never does anything; a ready persistent engine is adopted", () => {
  assert.equal(W.decideWarm({ mode: "off", engineReady: true }).action, "none");
  assert.equal(W.decideWarm({ mode: "auto", engineReady: true }).action, "adopt");
  assert.equal(W.decideWarm({ mode: "always", engineReady: true, saveData: true }).action, "adopt");
});

test("auto: data saver stops before any check; a first-time visitor never warms; a returning one checks the cache", () => {
  assert.equal(W.decideWarm({ mode: "auto", saveData: true, checked: false, returning: true }).action, "none");
  assert.deepEqual(W.decideWarm({ mode: "auto", saveData: false, checked: false }), { action: "none", consent: false, reason: "first visit" });
  assert.equal(W.decideWarm({ mode: "auto", saveData: false, checked: false, returning: true }).action, "check");
  assert.equal(W.decideWarm({ mode: "always", saveData: true, checked: false }).action, "check", "always still checks: a cached lane costs no data");
  assert.equal(W.decideWarm({ mode: "always", checked: false }).action, "check", "always does not wait for a returning visitor");
});

test("auto: init only for a cached lane the visitor consented to before (or no dense lane)", () => {
  const base = { mode: "auto", saveData: false, checked: true };
  assert.deepEqual(W.decideWarm(Object.assign({}, base, { lane: null, cached: true })), { action: "init", consent: false, reason: "no dense lane to download" });
  const ok = W.decideWarm(Object.assign({}, base, { lane, cached: true, consentedLane: "qwen3e" }));
  assert.equal(ok.action, "init");
  assert.equal(ok.consent, false);
  assert.equal(W.decideWarm(Object.assign({}, base, { lane, cached: true, consentedLane: "bge-small" })).action, "none");
  assert.equal(W.decideWarm(Object.assign({}, base, { lane, cached: true, consentedLane: null })).action, "none");
  assert.equal(W.decideWarm(Object.assign({}, base, { lane, cached: false, consentedLane: "qwen3e" })).action, "none", "never download in auto");
});

test("always: cached lanes init without consent; uncached ones download unless data saver is on", () => {
  const base = { mode: "always", checked: true, lane };
  assert.deepEqual(W.decideWarm(Object.assign({}, base, { cached: true, saveData: true })).action, "init");
  const dl = W.decideWarm(Object.assign({}, base, { cached: false, saveData: false }));
  assert.equal(dl.action, "init");
  assert.equal(dl.consent, true);
  assert.equal(W.decideWarm(Object.assign({}, base, { cached: false, saveData: true })).action, "none");
});

test("the storage keys are stable", () => {
  assert.equal(W.SEARCH_CONSENT_KEY, "eddie.search.consent");
  assert.equal(W.SEARCH_TIER_KEY, "eddie.search.tier");
  assert.equal(W.SEARCH_USED_KEY, "eddie.search.used");
});
