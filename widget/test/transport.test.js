// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const T = require("../src/lib/transport.js");

test("transport kind: service worker only with persist auto, a container and a secure context", () => {
  assert.equal(T.chooseTransportKind({ persist: "auto", hasServiceWorker: true, secureContext: true }), "sw");
  assert.equal(T.chooseTransportKind({ persist: "off", hasServiceWorker: true, secureContext: true }), "worker");
  assert.equal(T.chooseTransportKind({ persist: "auto", hasServiceWorker: false, secureContext: true }), "worker");
  assert.equal(T.chooseTransportKind({ persist: "auto", hasServiceWorker: true, secureContext: false }), "worker");
  // `isSecureContext` undefined (old browsers): let registration decide
  assert.equal(T.chooseTransportKind({ persist: "auto", hasServiceWorker: true }), "sw");
});

test("keepalive wanted while the modal is open, an answer streams or a request is pending", () => {
  assert.equal(T.keepaliveWanted({ open: false, streaming: false, pending: false }), false);
  assert.equal(T.keepaliveWanted({ open: true }), true);
  assert.equal(T.keepaliveWanted({ streaming: true }), true);
  assert.equal(T.keepaliveWanted({ pending: true }), true);
  assert.equal(T.keepaliveWanted(), false);
});

test("service worker state reuse: agent needs the same loaded model, search the same ready index", () => {
  assert.equal(T.canReuseAgent({ model: "Qwen3.5-2B-q4f32_1-MLC", loaded: true }, "Qwen3.5-2B-q4f32_1-MLC"), true);
  assert.equal(T.canReuseAgent({ model: "Qwen3.5-2B-q4f32_1-MLC", loaded: false }, "Qwen3.5-2B-q4f32_1-MLC"), false);
  assert.equal(T.canReuseAgent({ model: "Qwen3.5-0.8B-q4f32_1-MLC", loaded: true }, "Qwen3.5-2B-q4f32_1-MLC"), false);
  assert.equal(T.canReuseAgent(null, "x"), false);
  const ready = { phase: "ready", indexLoaded: true, indexUrl: "https://x/eddie/index.ed?v=1" };
  assert.equal(T.canReuseSearch(ready, "https://x/eddie/index.ed?v=1"), true);
  assert.equal(T.canReuseSearch(ready, "https://x/eddie/index.ed?v=2"), false, "a redeploy changes ?v=");
  assert.equal(T.canReuseSearch({ phase: "awaiting_consent", indexLoaded: true, indexUrl: ready.indexUrl }, ready.indexUrl), false);
  assert.equal(T.canReuseSearch(undefined, ready.indexUrl), false);
});

test("search stays page-side only when the page has WebGPU the service worker lacks", () => {
  assert.equal(T.searchStaysOnPage({ swOnnx: true, pageHasGpu: true, denseRuntime: "auto" }), false);
  assert.equal(T.searchStaysOnPage({ swOnnx: false, pageHasGpu: true, denseRuntime: "auto" }), true);
  assert.equal(T.searchStaysOnPage({ swOnnx: false, pageHasGpu: true, denseRuntime: "webgpu" }), true);
  assert.equal(T.searchStaysOnPage({ swOnnx: false, pageHasGpu: true, denseRuntime: "wasm" }), false);
  assert.equal(T.searchStaysOnPage({ swOnnx: false, pageHasGpu: false, denseRuntime: "auto" }), false);
});

/** A fake service worker end: answers hello/ping/state and echoes `echo` requests. */
function fakeServiceWorker(opts) {
  const o = opts || {};
  const ports = [];
  const sw = {
    posted: [],
    postMessage(msg, transfer) {
      sw.posted.push(msg);
      const port = transfer && transfer[0];
      if (!port) return;
      ports.push(port);
      port.onmessage = (e) => {
        const m = e.data;
        if (o.silent) return;
        if (m.type === "hello") port.postMessage({ type: "hello", requestId: m.requestId, ok: true, gpu: true, onnx: true, search: { phase: "idle" }, agent: { loaded: false } });
        else if (m.type === "ping") port.postMessage({ type: "pong", requestId: m.requestId });
        else if (m.type === "state") port.postMessage({ type: "state", requestId: m.requestId, gpu: true, search: { phase: "ready" }, agent: { loaded: true, model: "m" } });
        else if (m.type === "echo") port.postMessage({ type: "echo_result", requestId: m.requestId, value: m.value });
        else if (m.type === "fail") port.postMessage({ type: "error", requestId: m.requestId, message: "index not loaded yet" });
        else if (m.type === "stream") {
          port.postMessage({ type: "token", requestId: m.requestId, text: "a" });
          port.postMessage({ type: "done", requestId: m.requestId, answer: "a" });
        }
      };
    },
  };
  return { sw, ports, registration: { active: sw } };
}

test("ServiceWorkerTransport connects with a transferred port, answers calls and emits unsolicited messages", async () => {
  const f = fakeServiceWorker();
  const t = new T.ServiceWorkerTransport(f.registration, { kind: "search", version: "v1" });
  const hello = await t.connect();
  assert.equal(hello.ok, true);
  assert.deepEqual(f.sw.posted[0], { type: "connect", kind: "search", version: "v1" });
  const echo = await t.call("echo", { value: 42 }, { requestId: 7 });
  assert.equal(echo.value, 42);
  assert.equal(echo.requestId, 7);
  await assert.rejects(t.call("fail", {}, { requestId: 8 }), /index not loaded yet/);
  const got = [];
  t.on("status", (m) => got.push(m));
  f.ports[0].postMessage({ type: "status", state: "loading_wasm" });
  await new Promise((r) => setTimeout(r, 10));
  assert.deepEqual(got, [{ type: "status", state: "loading_wasm" }]);
  // Streaming replies keep the promise pending until the terminal message.
  const tokens = [];
  t.on("token", (m) => tokens.push(m.text));
  const done = await t.call("stream", {}, { requestId: 9 });
  assert.equal(done.type, "done");
  assert.deepEqual(tokens, ["a"]);
  t.terminate();
});

test("ServiceWorkerTransport: hello timeout rejects; a silent worker triggers reconnect + reset", async () => {
  const silent = fakeServiceWorker({ silent: true });
  const t0 = new T.ServiceWorkerTransport(silent.registration, { helloTimeoutMs: 30 });
  await assert.rejects(t0.connect(), /hello: no reply within 30 ms/);
  t0.terminate();

  // Live worker first, then it goes quiet (Chrome stopped it): ping fails,
  // reconnect opens a fresh channel and the widget is told to re-init.
  let quiet = false;
  const ports = [];
  const sw = {
    postMessage(msg, transfer) {
      const port = transfer[0];
      ports.push(port);
      port.onmessage = (e) => {
        const m = e.data;
        if (quiet && m.type === "ping") return;
        if (m.type === "hello") port.postMessage({ type: "hello", requestId: m.requestId, ok: true });
        if (m.type === "ping") port.postMessage({ type: "pong", requestId: m.requestId });
      };
    },
  };
  const t = new T.ServiceWorkerTransport({ active: sw }, { pingTimeoutMs: 30, helloTimeoutMs: 200 });
  await t.connect();
  const resets = [];
  t.on("reset", (m) => resets.push(m));
  assert.equal(await t.ensureAlive(), true);
  assert.equal(resets.length, 0);
  quiet = true;
  const pendingRejects = assert.rejects(t.call("echo", {}, { requestId: 1 }), /service worker restarted/);
  assert.equal(await t.ensureAlive(), true);
  await pendingRejects;
  assert.equal(resets.length, 1);
  assert.equal(ports.length, 2, "reconnect transferred a new port");
  t.terminate();
});

test("registerServiceWorker resolves once a worker is active, rejects on redundant, never uses .ready", async () => {
  const events = {};
  const sw = {
    state: "installing",
    addEventListener(type, fn) {
      events[type] = fn;
    },
    removeEventListener() {},
  };
  const registration = { installing: sw, active: null };
  let registered = null;
  const container = {
    register(url, opts) {
      registered = { url, opts };
      return Promise.resolve(registration);
    },
    get ready() {
      throw new Error("navigator.serviceWorker.ready must not be used");
    },
  };
  const p = T.registerServiceWorker({ container, url: "/eddie/eddie-sw.js?v=1", scope: "/eddie/", timeoutMs: 500 });
  await new Promise((r) => setTimeout(r, 5));
  assert.deepEqual(registered.opts, { type: "module", scope: "/eddie/", updateViaCache: "none" });
  sw.state = "activated";
  events.statechange();
  assert.equal(await p, registration);

  const sw2 = { state: "installing", addEventListener(t, fn) { events.c2 = fn; }, removeEventListener() {} };
  const p2 = T.registerServiceWorker({ container: { register: () => Promise.resolve({ installing: sw2 }) }, url: "/x", scope: "/", timeoutMs: 500 });
  await new Promise((r) => setTimeout(r, 5));
  sw2.state = "redundant";
  events.c2();
  await assert.rejects(p2, /redundant/);

  await assert.rejects(
    T.registerServiceWorker({ container: { register: () => new Promise(() => {}) }, url: "/x", scope: "/", timeoutMs: 20 }),
    /timed out/
  );
  await assert.rejects(
    T.registerServiceWorker({ container: { register: () => Promise.reject(new TypeError("ServiceWorker cannot be started")) }, url: "/x", scope: "/", timeoutMs: 20 }),
    /cannot be started/
  );
});
