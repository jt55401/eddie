// SPDX-License-Identifier: GPL-3.0-only

// Eddie agent engine, host-independent.
//
// Everything the agent worker does (WebLLM load, plan, ask/stream, abort with
// the drained-stream lock rule) behind an `env` the host supplies:
//
//   createAgentEngine({
//     post(message)     broadcast sink (unused today; progress goes to the loaders)
//     loadWebLLM()      -> Promise of the WebLLM module
//     now()             optional clock (tests)
//   })
//
// `engine.handle(msg, reply)` dispatches one protocol message; `reply` is
// the sink for that message's answers (progress/loaded for `load`,
// plan_result, token/done/aborted for `ask`, error). Message shapes are in
// widget/README.md ("Worker protocol", agent section).

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

  const EVIDENCE_CHARS = 700;
  const NOT_LOADED = "model not loaded";

  function createAgentEngine(env) {
    const lib = typeof EddieLib === "object" && EddieLib ? EddieLib : env.lib;
    const now = env.now || (() => (typeof performance === "object" && performance.now ? performance.now() : Date.now()));

    let webllm = null;
    let engine = null;
    let modelId = null;
    let loading = null; // { model, promise, waiters: [reply] }
    let active = null; // { requestId, aborted, reply }
    let queue = Promise.resolve();

    function handle(msg, reply) {
      const m = msg || {};
      const out = reply || env.post;
      switch (m.type) {
        case "load":
          return load(m, out);
        case "plan":
          return enqueue(() => plan(m, out), m.requestId, out);
        case "ask":
          return enqueue(() => ask(m, out), m.requestId, out);
        case "abort":
          abort(m);
          return Promise.resolve();
        default:
          postError(out, m.requestId, `unknown message type ${String(m.type)}`);
          return Promise.resolve();
      }
    }

    /** Snapshot for the service worker's `state` reply. */
    function snapshot() {
      return {
        model: modelId,
        loaded: !!engine,
        loading: loading ? loading.model : null,
        active: active ? active.requestId : null,
      };
    }

    function enqueue(fn, requestId, reply) {
      queue = queue
        .then(() => {
          console.debug("eddie agent engine: start", requestId);
          return fn();
        })
        .catch((err) => postError(reply, requestId, describe(err)))
        .then(() => console.debug("eddie agent engine: end", requestId));
      return queue;
    }

    async function load(msg, reply) {
      const model = String(msg.model || "");
      if (!model) {
        postError(reply, undefined, "load: model is required");
        return;
      }
      if (engine && modelId === model) {
        reply({ type: "loaded", model, loadMs: 0, cached: true });
        return;
      }
      if (loading) {
        // Another page (or an earlier message) is loading. Same model: join
        // it, the fan-out delivers progress and loaded/error to us too.
        // Different model: wait for it to settle, then load ours.
        const join = loading.model === model;
        if (join) loading.waiters.push(reply);
        try {
          await loading.promise;
        } catch (_) {
          // the fan-out already reported the error to the joined waiters
        }
        if (join) return;
        if (engine && modelId === model) {
          reply({ type: "loaded", model, loadMs: 0, cached: true });
          return;
        }
      }
      const job = { model, waiters: [reply], promise: null };
      const fanout = (message) => {
        for (const w of job.waiters) w(message);
      };
      job.promise = (async () => {
        const t0 = now();
        if (!webllm) {
          fanout({ type: "progress", text: "Loading the WebLLM runtime…", progress: 0 });
          webllm = await env.loadWebLLM();
        }
        if (engine) {
          try {
            await engine.unload();
          } catch (_) {
            // ignore
          }
          engine = null;
          modelId = null;
        }
        const created = await webllm.CreateMLCEngine(model, {
          initProgressCallback: (p) => {
            fanout({
              type: "progress",
              text: p && p.text ? p.text : "Loading model…",
              progress: p && typeof p.progress === "number" ? p.progress : null,
            });
          },
        });
        engine = created;
        modelId = model;
        fanout({ type: "loaded", model, loadMs: Math.round(now() - t0) });
      })();
      loading = job;
      try {
        await job.promise;
      } catch (err) {
        engine = null;
        modelId = null;
        fanout({ type: "error", requestId: undefined, message: describe(err) });
      } finally {
        if (loading === job) loading = null;
      }
    }

    function requireEngine() {
      if (!engine) throw new Error(NOT_LOADED);
    }

    async function plan(msg, reply) {
      requireEngine();
      const question = String(msg.question || "").trim();
      const site = String(msg.site || "this website");
      const t0 = now();
      const replyMsg = await engine.chat.completions.create({
        messages: [
          { role: "system", content: lib.planPrompt(site) },
          { role: "user", content: question },
        ],
        temperature: 0,
        max_tokens: 100,
        response_format: { type: "json_object", schema: JSON.stringify(lib.PLAN_SCHEMA) },
        extra_body: { enable_thinking: false },
      });
      const content = replyMsg && replyMsg.choices && replyMsg.choices[0] && replyMsg.choices[0].message ? replyMsg.choices[0].message.content : "";
      const queries = lib.parsePlan(content, question);
      reply({ type: "plan_result", requestId: msg.requestId, queries, ms: Math.round(now() - t0) });
    }

    async function ask(msg, reply) {
      requireEngine();
      const requestId = msg.requestId;
      const question = String(msg.question || "").trim();
      const site = String(msg.site || "this website");
      const evidence = Array.isArray(msg.evidence) ? msg.evidence.filter((e) => e && e.url) : [];
      if (evidence.length === 0) {
        reply({
          type: "done",
          requestId,
          answer: lib.NOHIT,
          citations: [],
          nohit: true,
          raw: "",
          usage: { ttftMs: 0, totalMs: 0, tps: null, completionTokens: 0 },
        });
        return;
      }
      active = { requestId, aborted: false, reply };
      const t0 = now();
      let first = 0;
      let text = "";
      let usage = null;
      try {
        const stream = await engine.chat.completions.create({
          messages: [
            { role: "system", content: lib.answerPrompt(site) },
            { role: "user", content: lib.sourcesPrompt(evidence, question, EVIDENCE_CHARS) },
          ],
          stream: true,
          stream_options: { include_usage: true },
          temperature: 0,
          frequency_penalty: 0.5,
          presence_penalty: 0,
          max_tokens: 220,
          extra_body: { enable_thinking: false },
        });
        // Never break out of this loop: WebLLM releases its generation lock at
        // the end of the async generator, and an early exit skips that release,
        // hanging every later completion. After interruptGenerate() the stream
        // ends by itself within one decode step; drop the tokens until then.
        for await (const chunk of stream) {
          if (active.aborted) continue;
          const delta = chunk && chunk.choices && chunk.choices[0] && chunk.choices[0].delta ? chunk.choices[0].delta.content : null;
          if (delta) {
            if (!first) first = now();
            text += delta;
            reply({ type: "token", requestId, text: delta });
          }
          if (chunk && chunk.usage) usage = chunk.usage;
        }
      } finally {
        const wasAborted = active && active.aborted;
        active = null;
        if (wasAborted) {
          reply({ type: "aborted", requestId });
          return;
        }
      }
      const processed = lib.postProcessAnswer(text, evidence);
      const totalMs = Math.round(now() - t0);
      reply({
        type: "done",
        requestId,
        answer: processed.answer,
        citations: processed.citations,
        nohit: processed.nohit,
        raw: text,
        usage: {
          ttftMs: first ? Math.round(first - t0) : totalMs,
          totalMs,
          tps: usage && usage.extra && typeof usage.extra.decode_tokens_per_s === "number" ? Math.round(usage.extra.decode_tokens_per_s) : null,
          completionTokens: usage ? usage.completion_tokens : null,
        },
      });
    }

    function abort(msg) {
      console.debug("eddie agent engine: abort", msg.requestId, active ? active.requestId : null);
      if (!active) return;
      if (msg.requestId != null && msg.requestId !== active.requestId) return;
      active.aborted = true;
      try {
        if (engine) engine.interruptGenerate();
      } catch (err) {
        console.warn("eddie agent: interrupt failed", err);
      }
    }

    /** Abort the active run if it belongs to a page that went away. */
    function abortIfOwner(reply) {
      if (active && active.reply === reply) abort({ requestId: active.requestId });
    }

    function postError(reply, requestId, message) {
      reply({ type: "error", requestId: requestId == null ? undefined : requestId, message });
    }

    return { handle, state: snapshot, abortIfOwner };
  }

  function describe(err) {
    if (err == null) return "unknown error";
    if (typeof err === "string") return err;
    return err.message || String(err);
  }

  /** True for the engine's "not loaded" replies (the client re-runs load). */
  function isModelNotLoadedMessage(message) {
    return typeof message === "string" && message.indexOf(NOT_LOADED) === 0;
  }

  return { createAgentEngine, AGENT_NOT_LOADED: NOT_LOADED, isModelNotLoadedMessage };
});
