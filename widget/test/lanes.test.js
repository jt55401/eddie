// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const L = Object.assign({}, require("../src/lib/lanes.js"), require("../src/lib/copy.js"));

const minilm = {
  id: "minilm", model: "sentence-transformers/multi-qa-MiniLM-L6-cos-v1", family: "bert", dim: 384,
  pooling: "cls", normalize: true, revision: "abc", quant: "int8",
  runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "model.safetensors"] },
};
const qwen = {
  id: "qwen3e", model: "Qwen/Qwen3-Embedding-0.6B", family: "qwen3", dim: 1024, pooling: "last", normalize: true,
  runtime: { kind: "webgpu-onnx", repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX", dtype: "q4", dtype_f16: "q4f16", pooling: "last_token" },
};

test("auto prefers webgpu when available, wasm otherwise", () => {
  const m = { dense: [minilm, qwen] };
  assert.deepEqual(L.chooseDenseLanes(m, { denseRuntime: "auto", hasWebGpu: true }).candidates.map((l) => l.id), ["qwen3e", "minilm"]);
  assert.deepEqual(L.chooseDenseLanes(m, { denseRuntime: "auto", hasWebGpu: false }).candidates.map((l) => l.id), ["minilm"]);
});

test("forced runtimes", () => {
  const m = { dense: [minilm, qwen] };
  assert.deepEqual(L.chooseDenseLanes(m, { denseRuntime: "wasm", hasWebGpu: true }).candidates.map((l) => l.id), ["minilm"]);
  assert.deepEqual(L.chooseDenseLanes(m, { denseRuntime: "webgpu", hasWebGpu: true }).candidates.map((l) => l.id), ["qwen3e"]);
  const none = L.chooseDenseLanes(m, { denseRuntime: "webgpu", hasWebGpu: false });
  assert.equal(none.candidates.length, 0);
  assert.equal(none.reason, "no WebGPU adapter");
});

test("empty and unrunnable manifests explain why", () => {
  assert.equal(L.chooseDenseLanes({ dense: [] }, {}).reason, "index has no dense lane");
  assert.equal(L.chooseDenseLanes({}, {}).reason, "index has no dense lane");
  assert.equal(L.chooseDenseLanes({ dense: [qwen] }, { denseRuntime: "auto", hasWebGpu: false }).reason, "no WebGPU adapter");
  assert.equal(L.chooseDenseLanes({ dense: [qwen] }, { denseRuntime: "wasm", hasWebGpu: true }).reason, "index has no wasm-candle lane");
});

test("lane files, repo, revision", () => {
  assert.deepEqual(L.laneFiles(minilm), ["config.json", "tokenizer.json", "model.safetensors"]);
  assert.deepEqual(L.laneFiles({ runtime: { kind: "wasm-candle" } }), L.DEFAULT_WASM_FILES);
  assert.deepEqual(L.laneFiles(qwen), []);
  assert.equal(L.laneRepo(minilm), "sentence-transformers/multi-qa-MiniLM-L6-cos-v1");
  assert.equal(L.laneRepo(qwen), "onnx-community/Qwen3-Embedding-0.6B-ONNX");
  assert.equal(L.laneRevision(minilm), "abc");
  assert.equal(L.laneRevision(qwen), "main");
  // The manifest pins lane.model, not the ONNX repo a webgpu lane downloads from.
  assert.equal(L.laneRevision(Object.assign({}, qwen, { revision: "97b0c614" })), "main");
});

test("dtype picks f16 only with shader-f16", () => {
  assert.equal(L.pickDtype(qwen.runtime, true), "q4f16");
  assert.equal(L.pickDtype(qwen.runtime, false), "q4");
  assert.equal(L.pickDtype({ dtype: "q8" }, true), "q8");
});

test("download sizes and formatting", () => {
  assert.equal(L.laneDownloadBytes(minilm), 91e6);
  assert.equal(L.laneDownloadBytes(qwen), 900e6);
  assert.equal(L.laneDownloadBytes({ model: "someone/unknown", runtime: { kind: "wasm-candle" } }), null);
  assert.equal(L.formatBytes(91e6), "91 MB");
  assert.equal(L.formatBytes(1.2e9), "1.2 GB");
  assert.equal(L.formatBytes(700e3), "700 KB");
  assert.equal(L.formatBytes(null), "unknown size");
});

test("consent copy states size, honours save-data and overrides", () => {
  const text = L.consentCopy({ sizeBytes: 91e6, model: "MiniLM" });
  assert.match(text, /91 MB/);
  assert.match(text, /MiniLM/);
  assert.doesNotMatch(text, /Data saver/);
  assert.match(L.consentCopy({ sizeBytes: 91e6, saveData: true }), /^Data saver is on\./);
  assert.match(L.consentCopy({ sizeBytes: null }), /size unknown/);
  assert.equal(L.consentCopy({ sizeBytes: 570e6, model: "bge-m3", consentText: "Get {model} ({size})?" }), "Get bge-m3 (570 MB)?");
});

test("degraded notice names the missing arm", () => {
  assert.equal(L.degradedNotice({ dense: true, sparse: true, bm25: true }, []), null);
  assert.equal(L.degradedNotice({ dense: false, sparse: false, bm25: true }, ["dense: index has no dense lane"]), null);
  assert.match(L.degradedNotice({ dense: false, sparse: false, bm25: true }, ["dense: no query vector (no runnable embedder)"]), /^Keyword-only results/);
  assert.match(L.degradedNotice({ dense: false, sparse: true, bm25: true }, ["dense: model failed"]), /dense model/);
  assert.match(L.degradedNotice({ dense: true, sparse: false, bm25: true }, ["sparse: no query terms (tokenizer not loaded)"]), /sparse/);
  assert.deepEqual(L.filterDesignDegraded(["dense: index has no dense lane", "sparse: x"]), ["sparse: x"]);
});

test("wasm lanes the WASM loader cannot run are skipped with a reason", () => {
  const bin = Object.assign({}, minilm, { id: "bin", runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "pytorch_model.bin"] } });
  const sharded = Object.assign({}, minilm, { id: "sharded", runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "model-00001-of-00002.safetensors", "model-00002-of-00002.safetensors"] } });
  const xlmr = Object.assign({}, minilm, { id: "xlmr", family: "xlm-roberta" });
  const noTok = Object.assign({}, minilm, { id: "notok", runtime: { kind: "wasm-candle", files: ["config.json", "model.safetensors"] } });
  assert.equal(L.wasmLaneProblem(minilm), null);
  assert.match(L.wasmLaneProblem(bin), /single model\.safetensors, not pytorch_model\.bin/);
  assert.match(L.wasmLaneProblem(sharded), /model-00001-of-00002\.safetensors/);
  assert.match(L.wasmLaneProblem(xlmr), /xlm-roberta/);
  assert.match(L.wasmLaneProblem(noTok), /tokenizer\.json/);
  const choice = L.chooseDenseLanes({ dense: [bin, sharded, minilm] }, { denseRuntime: "auto", hasWebGpu: false });
  assert.deepEqual(choice.candidates.map((l) => l.id), ["minilm"]);
  assert.deepEqual(choice.skipped.map((s) => s.lane.id), ["bin", "sharded"]);
  const none = L.chooseDenseLanes({ dense: [bin] }, { denseRuntime: "wasm" });
  assert.equal(none.candidates.length, 0);
  assert.match(none.reason, /WASM loader/);
});

test("site-bundled lanes: origin, file URLs next to the index, and a cache name that never collides with the repo copy", () => {
  const site = { id: "bge", model: "BAAI/bge-small-en-v1.5", runtime: { kind: "wasm-candle", files: ["config.json", "tokenizer.json", "model.safetensors"], base_url: "models/bge/" } };
  const hf = { id: "bge", model: "BAAI/bge-small-en-v1.5", runtime: { kind: "wasm-candle", files: ["config.json"] } };
  assert.equal(L.laneOrigin(site), "site");
  assert.equal(L.laneOrigin(hf), "huggingface");
  assert.equal(L.laneBaseUrl(hf), null);
  assert.equal(L.siteModelUrl(site, "model.safetensors", "https://x.test/eddie/index.ed?v=1"), "https://x.test/eddie/models/bge/model.safetensors");
  assert.equal(L.siteModelUrl({ runtime: { base_url: "models/bge" } }, "/config.json", "https://x.test/eddie/index.ed"), "https://x.test/eddie/models/bge/config.json");
  assert.equal(L.siteModelUrl(hf, "config.json", "https://x.test/eddie/index.ed"), null);
  assert.equal(L.laneFileName(site, "model.safetensors"), "@site/model.safetensors");
  assert.equal(L.laneFileName(hf, "model.safetensors"), "model.safetensors");
});

test("sidecar lookup follows manifest.sidecars per scope and lane", () => {
  const manifest = {
    sidecars: [
      { file: "index.bge.ed", lane: "bge", scope: "qa", bytes: 43643 },
      { file: "index.qwen3e.ed", lane: "qwen3e", scope: "chunks", bytes: 533650 },
      { file: "index.qwen3e.ed", lane: "qwen3e", scope: "qa", bytes: 533650 },
    ],
  };
  assert.equal(L.sidecarFor(manifest, "chunks", "qwen3e").file, "index.qwen3e.ed");
  assert.equal(L.sidecarFor(manifest, "chunks", "bge"), null, "wasm-candle chunk vectors stay in the core file");
  assert.equal(L.sidecarFor(manifest, "qa", "bge").file, "index.bge.ed");
  assert.equal(L.laneSidecarBytes(manifest, "qwen3e"), 533650);
  assert.equal(L.laneSidecarBytes(manifest, "bge"), 0);
  assert.equal(L.laneSidecarBytes({}, "bge"), 0);
});

test("consent copy names the size, the origin and the sidecar bytes", () => {
  assert.equal(
    L.consentCopy({ sizeBytes: 67458275, model: "bge-small-en-v1.5", origin: "site" }),
    "Semantic search runs bge-small-en-v1.5 in your browser. That needs a one-time 67 MB download from this site, kept in your browser's cache for next time."
  );
  assert.equal(
    L.consentCopy({ sizeBytes: 900e6, model: "Qwen3-Embedding-0.6B-ONNX", origin: "huggingface", sidecarBytes: 533650 }),
    "Semantic search runs Qwen3-Embedding-0.6B-ONNX in your browser. That needs a one-time 900 MB download from huggingface.co, plus 534 KB of index vectors from this site, kept in your browser's cache for next time."
  );
  assert.match(L.consentCopy({ sizeBytes: null, model: "m" }), /a one-time download \(size unknown\) from huggingface\.co/);
  assert.equal(L.consentCopy({ sizeBytes: 1e6, model: "m", origin: "site", consentText: "{model}: {size} from {origin}" }), "m: 1 MB from this site");
});

test("laneDownloadBytes prefers the manifest's runtime.bytes over the table estimate", () => {
  const bundled = { model: "BAAI/bge-small-en-v1.5", runtime: { kind: "wasm-candle", base_url: "models/bge/", bytes: 67458275 } };
  assert.equal(L.laneDownloadBytes(bundled), 67458275);
  assert.equal(L.laneDownloadBytes({ model: "BAAI/bge-small-en-v1.5", runtime: { kind: "wasm-candle" } }), 134e6, "no bytes: table estimate");
  assert.equal(L.laneDownloadBytes({ model: "x/unknown", runtime: { kind: "wasm-candle", bytes: 0 } }), null, "zero or absent bytes and unknown repo: unknown");
});
