// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");

const lib = Object.assign({}, require("../src/lib/agent.js"), require("../src/lib/agent-llm.js"));
const AE = require("../src/lib/agent-engine.js");

/** Fake WebLLM: a scripted engine whose stream honours interruptGenerate the way WebLLM does. */
function fakeWebLLM(opts) {
  const o = opts || {};
  const log = [];
  const engine = {
    interrupted: false,
    unloaded: 0,
    interruptGenerate() {
      this.interrupted = true;
      log.push("interrupt");
    },
    async unload() {
      this.unloaded++;
    },
    chat: {
      completions: {
        create: async (req) => {
          log.push(req.stream ? "stream" : "plan");
          if (!req.stream) return { choices: [{ message: { content: o.planReply || '{"queries":["rust tools","companies"]}' } }] };
          const tokens = o.tokens || ["Jason ", "writes ", "Rust [1]."];
          const self = engine;
          return {
            async *[Symbol.asyncIterator]() {
              for (const t of tokens) {
                if (self.interrupted) break; // WebLLM ends the stream after an interrupt
                await new Promise((r) => setTimeout(r, 1));
                yield { choices: [{ delta: { content: t } }] };
              }
              yield { choices: [{ delta: {} }], usage: { completion_tokens: tokens.length, extra: { decode_tokens_per_s: 50 } } };
              log.push("stream-end");
            },
          };
        },
      },
    },
  };
  return {
    log,
    engine,
    module: {
      CreateMLCEngine: async (model, cfg) => {
        log.push("create " + model);
        cfg.initProgressCallback({ text: "Loading", progress: 0.5 });
        if (o.failLoad) throw new Error("no GPU");
        return engine;
      },
    },
  };
}

function makeEngine(opts) {
  const w = fakeWebLLM(opts);
  const posted = [];
  const engine = AE.createAgentEngine({ lib, post: (m) => posted.push(m), loadWebLLM: async () => w.module });
  return { engine, posted, w };
}

const EVIDENCE = [{ title: "Bio", url: "/about/", text: "Jason writes Rust." }];

test("load reports progress and loaded; a repeat load is answered from cache; state reflects it", async () => {
  const { engine, w } = makeEngine();
  const out = [];
  assert.deepEqual(engine.state(), { model: null, loaded: false, loading: null, active: null });
  await engine.handle({ type: "load", model: "m1" }, (m) => out.push(m));
  assert.deepEqual(out.map((m) => m.type), ["progress", "progress", "loaded"]);
  assert.equal(out[2].model, "m1");
  assert.deepEqual(engine.state(), { model: "m1", loaded: true, loading: null, active: null });
  out.length = 0;
  await engine.handle({ type: "load", model: "m1" }, (m) => out.push(m));
  assert.deepEqual(out, [{ type: "loaded", model: "m1", loadMs: 0, cached: true }]);
  assert.equal(w.log.filter((l) => l.startsWith("create")).length, 1);
  await engine.handle({ type: "load", model: "m2" }, (m) => out.push(m));
  assert.equal(w.engine.unloaded, 1, "switching models unloads the old engine first");
  assert.equal(engine.state().model, "m2");
});

test("two pages loading the same model share one load and both hear loaded", async () => {
  const { engine, w } = makeEngine();
  const a = [];
  const b = [];
  const pa = engine.handle({ type: "load", model: "m1" }, (m) => a.push(m));
  const pb = engine.handle({ type: "load", model: "m1" }, (m) => b.push(m));
  await Promise.all([pa, pb]);
  assert.equal(w.log.filter((l) => l.startsWith("create")).length, 1);
  assert.equal(a[a.length - 1].type, "loaded");
  assert.equal(b[b.length - 1].type, "loaded");
});

test("a failed load reports error and leaves the engine unloaded", async () => {
  const { engine } = makeEngine({ failLoad: true });
  const out = [];
  await engine.handle({ type: "load", model: "m1" }, (m) => out.push(m));
  assert.equal(out[out.length - 1].type, "error");
  assert.match(out[out.length - 1].message, /no GPU/);
  assert.equal(engine.state().loaded, false);
});

test("plan and ask before load answer with the not-loaded error the client retries on", async () => {
  const { engine } = makeEngine();
  const out = [];
  await engine.handle({ type: "plan", requestId: 1, question: "q" }, (m) => out.push(m));
  assert.equal(out[0].type, "error");
  assert.equal(out[0].requestId, 1);
  assert.equal(AE.isModelNotLoadedMessage(out[0].message), true);
  await engine.handle({ type: "ask", requestId: 2, question: "q", evidence: EVIDENCE }, (m) => out.push(m));
  assert.equal(AE.isModelNotLoadedMessage(out[1].message), true);
});

test("plan parses the reply; ask streams tokens then done with citations", async () => {
  const { engine } = makeEngine();
  await engine.handle({ type: "load", model: "m1" }, () => {});
  const out = [];
  await engine.handle({ type: "plan", requestId: 1, question: "what does jason write?", site: "x" }, (m) => out.push(m));
  assert.equal(out[0].type, "plan_result");
  assert.deepEqual(out[0].queries, ["rust tools", "companies"]);
  await engine.handle({ type: "ask", requestId: 2, question: "what does jason write?", site: "x", evidence: EVIDENCE }, (m) => out.push(m));
  const tokens = out.filter((m) => m.type === "token").map((m) => m.text);
  assert.deepEqual(tokens, ["Jason ", "writes ", "Rust [1]."]);
  const done = out[out.length - 1];
  assert.equal(done.type, "done");
  assert.equal(done.answer, "Jason writes Rust [1].");
  assert.deepEqual(done.citations, [{ n: 1, url: "/about/", title: "Bio" }]);
  assert.equal(done.usage.tps, 50);
  assert.equal(done.nohit, false);
});

test("ask without evidence answers nohit at once", async () => {
  const { engine } = makeEngine();
  await engine.handle({ type: "load", model: "m1" }, () => {});
  const out = [];
  await engine.handle({ type: "ask", requestId: 3, question: "q", evidence: [] }, (m) => out.push(m));
  assert.equal(out[0].type, "done");
  assert.equal(out[0].nohit, true);
  assert.equal(out[0].answer, lib.NOHIT);
});

test("abort drains the stream (lock stays released) and reports aborted; later asks still run", async () => {
  const { engine, w } = makeEngine({ tokens: ["a", "b", "c", "d", "e"] });
  await engine.handle({ type: "load", model: "m1" }, () => {});
  const out = [];
  const ask = engine.handle({ type: "ask", requestId: 5, question: "q", evidence: EVIDENCE }, (m) => out.push(m));
  await new Promise((r) => setTimeout(r, 3));
  assert.equal(engine.state().active, 5);
  await engine.handle({ type: "abort", requestId: 5 });
  await ask;
  assert.ok(w.log.includes("interrupt"));
  assert.ok(w.log.includes("stream-end"), "the generator ran to completion");
  assert.equal(out[out.length - 1].type, "aborted");
  assert.equal(out[out.length - 1].requestId, 5);
  assert.ok(out.filter((m) => m.type === "token").length < 5);
  assert.equal(engine.state().active, null);
  // An abort for a different request is ignored; a later ask completes.
  await engine.handle({ type: "abort", requestId: 99 });
  w.engine.interrupted = false;
  const out2 = [];
  await engine.handle({ type: "ask", requestId: 6, question: "q", evidence: EVIDENCE }, (m) => out2.push(m));
  assert.equal(out2[out2.length - 1].type, "done");
});

test("abortIfOwner stops only the run that belongs to the departed page", async () => {
  const { engine, w } = makeEngine({ tokens: ["a", "b", "c", "d"] });
  await engine.handle({ type: "load", model: "m1" }, () => {});
  const pageA = () => {};
  const pageB = () => {};
  const run = engine.handle({ type: "ask", requestId: 7, question: "q", evidence: EVIDENCE }, pageA);
  await new Promise((r) => setTimeout(r, 2));
  engine.abortIfOwner(pageB);
  assert.equal(w.engine.interrupted, false);
  engine.abortIfOwner(pageA);
  assert.equal(w.engine.interrupted, true);
  await run;
});
