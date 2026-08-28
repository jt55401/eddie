// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { parseWidgetConfig } = require("../src/lib/config.js");

const from = (obj) => parseWidgetConfig((k) => (k in obj ? obj[k] : null));

test("defaults when nothing is set", () => {
  const c = from({});
  assert.equal(c.indexUrl, "");
  assert.equal(c.position, "bottom-right");
  assert.equal(c.theme, "auto");
  assert.equal(c.qaMode, "auto");
  assert.equal(c.topK, 8);
  assert.equal(c.answerTopK, 5);
  assert.equal(c.agentMode, "auto");
  assert.equal(c.agentModel, "auto");
  assert.equal(c.denseRuntime, "auto");
  assert.equal(c.consentText, "");
  assert.equal(c.offsetX, 0);
});

test("reads every documented attribute", () => {
  const c = from({
    "data-index-url": "/eddie/index.ed?v=abc",
    "data-position": "TOP-LEFT",
    "data-theme": "Dark",
    "data-offset-x": "12.7",
    "data-offset-y": "-4",
    "data-qa-mode": "always",
    "data-qa-subject": " Jason Grey ",
    "data-top-k": "5",
    "data-answer-top-k": "3",
    "data-agent-mode": "off",
    "data-agent-model": "Qwen3.5-4B-q4f32_1-MLC",
    "data-dense-runtime": "webgpu",
    "data-consent-text": "Download {size}?",
  });
  assert.equal(c.indexUrl, "/eddie/index.ed?v=abc");
  assert.equal(c.position, "top-left");
  assert.equal(c.theme, "dark");
  assert.equal(c.offsetX, 12);
  assert.equal(c.offsetY, -4);
  assert.equal(c.qaMode, "always");
  assert.equal(c.qaSubject, "Jason Grey");
  assert.equal(c.topK, 5);
  assert.equal(c.answerTopK, 3);
  assert.equal(c.agentMode, "off");
  assert.equal(c.agentModel, "Qwen3.5-4B-q4f32_1-MLC");
  assert.equal(c.denseRuntime, "webgpu");
  assert.equal(c.consentText, "Download {size}?");
});

test("invalid values fall back", () => {
  const c = from({
    "data-position": "middle",
    "data-theme": "sepia",
    "data-top-k": "0",
    "data-answer-top-k": "abc",
    "data-agent-mode": "maybe",
    "data-dense-runtime": "cuda",
    "data-offset-x": "wide",
  });
  assert.equal(c.position, "bottom-right");
  assert.equal(c.theme, "auto");
  assert.equal(c.topK, 8);
  assert.equal(c.answerTopK, 5);
  assert.equal(c.agentMode, "auto");
  assert.equal(c.denseRuntime, "auto");
  assert.equal(c.offsetX, 0);
});
