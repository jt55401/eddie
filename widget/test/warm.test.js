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

test("auto: data saver stops before any check; otherwise check the cache first", () => {
  assert.equal(W.decideWarm({ mode: "auto", saveData: true, checked: false }).action, "none");
  assert.equal(W.decideWarm({ mode: "auto", saveData: false, checked: false }).action, "check");
  assert.equal(W.decideWarm({ mode: "always", saveData: true, checked: false }).action, "check", "always still checks: a cached lane costs no data");
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

test("the consent key is stable", () => {
  assert.equal(W.SEARCH_CONSENT_KEY, "eddie.search.consent");
});
