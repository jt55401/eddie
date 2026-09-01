// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const B = require("../src/lib/boot.js");

test("boot layout: same placement and theme rules as the full widget, invalid values fall back", () => {
  const attrs = { "data-position": "Top-Left", "data-theme": "DARK", "data-offset-x": "12", "data-offset-y": "-3.7", "data-warm": "always" };
  assert.deepEqual(B.bootLayout((n) => attrs[n] || null), { position: "top-left", theme: "dark", offsetX: 12, offsetY: -3, warm: "always" });
  assert.deepEqual(B.bootLayout(() => null), { position: "bottom-right", theme: "auto", offsetX: 0, offsetY: 0, warm: "auto" });
  assert.deepEqual(B.bootLayout((n) => ({ "data-position": "middle", "data-theme": "blue", "data-offset-x": "x", "data-warm": "maybe" })[n] || null), {
    position: "bottom-right",
    theme: "auto",
    offsetX: 0,
    offsetY: 0,
    warm: "auto",
  });
});

test("first-time visitors load the widget on interaction only; returning ones at idle", () => {
  assert.equal(B.decideBoot({ warm: "auto" }).action, "interaction");
  assert.equal(B.decideBoot({ warm: "auto", used: true }).action, "idle");
  assert.equal(B.decideBoot({ warm: "auto", consented: true }).action, "idle");
  assert.equal(B.decideBoot({ warm: "always" }).action, "idle");
});

test("data saver, prefers-reduced-data and warm=off never preload, whatever the history", () => {
  for (const flag of ["saveData", "reducedData"]) {
    assert.equal(B.decideBoot({ warm: "always", consented: true, used: true, [flag]: true }).action, "interaction", flag);
  }
  assert.equal(B.decideBoot({ warm: "off", consented: true, used: true }).action, "interaction");
  assert.equal(B.decideBoot({ warm: "off", consented: true }).reason, "warm is off");
});

test("the full widget URL sits next to the boot script and carries its ?v= (or the index's)", () => {
  assert.equal(B.widgetScriptUrl("https://x.test/eddie/eddie-boot.js?v=abc", "/eddie/index.ed?v=zzz"), "https://x.test/eddie/eddie-widget.js?v=abc");
  assert.equal(B.widgetScriptUrl("https://x.test/eddie/eddie-boot.js", "/eddie/index.ed?v=1a2b"), "https://x.test/eddie/eddie-widget.js?v=1a2b");
  assert.equal(B.widgetScriptUrl("https://x.test/assets/e/eddie-boot.js#frag", ""), "https://x.test/assets/e/eddie-widget.js");
  // Without a stamp (loaded from src/) the index version is still the last
  // resort, so a deployment that predates the asset hash keeps working; in a
  // real bundle EDDIE_ASSET_VERSION sits between those two.
  assert.equal(B.bootVersionOf("/eddie/index.ed?x=1&v=a%2Fb"), "a/b");
  assert.equal(B.bootVersionOf("/eddie/index.ed"), null);
});

test("storage keys match lib/warm.js", () => {
  const W = require("../src/lib/warm.js");
  assert.equal(B.SEARCH_CONSENT_KEY, W.SEARCH_CONSENT_KEY);
  assert.equal(B.SEARCH_USED_KEY, W.SEARCH_USED_KEY);
});
