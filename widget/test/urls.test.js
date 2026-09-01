// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const u = require("../src/lib/urls.js");

test("baseUrlOf strips file, query and hash", () => {
  assert.equal(u.baseUrlOf("https://x.test/blog/eddie/eddie-widget.js?v=1#a"), "https://x.test/blog/eddie/");
  assert.equal(u.baseUrlOf("https://x.test/eddie-widget.js"), "https://x.test/");
});

test("versionOf reads the v parameter", () => {
  assert.equal(u.versionOf("/eddie/index.ed?v=abc123"), "abc123");
  assert.equal(u.versionOf("/eddie/index.ed?x=1&v=9"), "9");
  assert.equal(u.versionOf("/eddie/index.ed"), null);
  assert.equal(u.versionOf("/eddie/index.ed?v="), null);
  assert.equal(u.versionOf(null), null);
});

test("withVersion appends only when missing", () => {
  assert.equal(u.withVersion("/eddie/index.ed", "abc"), "/eddie/index.ed?v=abc");
  assert.equal(u.withVersion("/eddie/index.ed?x=1", "abc"), "/eddie/index.ed?x=1&v=abc");
  assert.equal(u.withVersion("/eddie/index.ed?v=old", "abc"), "/eddie/index.ed?v=old");
  assert.equal(u.withVersion("/eddie/index.ed#frag", "a b"), "/eddie/index.ed?v=a%20b#frag");
  assert.equal(u.withVersion("/eddie/index.ed", null), "/eddie/index.ed");
});

test("assetUrl joins base and busts cache", () => {
  assert.equal(u.assetUrl("https://x.test/eddie/", "eddie.wasm", "v1"), "https://x.test/eddie/eddie.wasm?v=v1");
  assert.equal(u.assetUrl("https://x.test/eddie", "eddie-wasm.js", null), "https://x.test/eddie/eddie-wasm.js");
  assert.equal(u.assetUrl("", "eddie-wasm.js", null), "eddie-wasm.js");
});

test("ASSET_VERSION is the build stamp, absent outside a bundle", () => {
  // widget/build.sh prepends `const EDDIE_ASSET_VERSION = "<hash>";` to every
  // bundle; loaded straight from src/ there is no stamp and no `?v=`, which
  // is also what a hand-assembled deployment gets.
  assert.equal(u.ASSET_VERSION, null);
  assert.equal(u.assetUrl("/eddie/", "eddie-lite.wasm", u.ASSET_VERSION), "/eddie/eddie-lite.wasm");

  // The two versions are independent: runtime assets follow the build stamp,
  // the index and its sidecars follow the index's own `?v=`.
  const stamp = "90691e60079e";
  assert.equal(u.assetUrl("/eddie/", "eddie-lite.wasm", stamp), "/eddie/eddie-lite.wasm?v=" + stamp);
  assert.equal(u.withVersion("/eddie/index.qwen3e.ed", "1756700000"), "/eddie/index.qwen3e.ed?v=1756700000");
});

test("hfFileUrl pins the revision and encodes paths", () => {
  assert.equal(
    u.hfFileUrl("sentence-transformers/multi-qa-MiniLM-L6-cos-v1", "abc", "model.safetensors"),
    "https://huggingface.co/sentence-transformers/multi-qa-MiniLM-L6-cos-v1/resolve/abc/model.safetensors"
  );
  assert.equal(u.hfFileUrl("org/repo", null, "onnx/model q4.onnx"), "https://huggingface.co/org/repo/resolve/main/onnx/model%20q4.onnx");
});

test("cacheKey and timeouts", () => {
  assert.equal(u.cacheKey("org/repo", "sha1", "tokenizer.json"), "org/repo@sha1/tokenizer.json");
  assert.equal(u.cacheKey("org/repo", null, "x"), "org/repo@main/x");
  assert.equal(u.timeoutForFile("model.safetensors"), 600000);
  assert.equal(u.timeoutForFile("onnx/model_q4.onnx_data"), 600000);
  assert.equal(u.timeoutForFile("config.json"), 60000);
  assert.equal(u.isWeightsFile("tokenizer.json"), false);
});

test("cacheKeyFromUrl inverts hfFileUrl and rejects other hosts", () => {
  const url = u.hfFileUrl("onnx-community/Qwen3-Embedding-0.6B-ONNX", "main", "onnx/model_q4.onnx");
  assert.equal(u.cacheKeyFromUrl(url), "onnx-community/Qwen3-Embedding-0.6B-ONNX@main/onnx/model_q4.onnx");
  assert.equal(u.cacheKeyFromUrl(u.hfFileUrl("org/repo", "abc", "a b.json") + "?download=true"), "org/repo@abc/a b.json");
  assert.equal(u.cacheKeyFromUrl("https://cdn.jsdelivr.net/npm/x/model.onnx"), null);
  assert.equal(u.cacheKeyFromUrl("/models/org/repo/config.json"), null);
  assert.equal(u.cacheKeyFromUrl(null), null);
});
