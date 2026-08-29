// SPDX-License-Identifier: GPL-3.0-only

// Eddie agent worker (module worker, created on the first "Ask").
//
// Runs WebLLM in the worker; retrieval stays in the widget, which owns the
// search worker and passes evidence in. widget/build.sh concatenates
// widget/src/lib/agent.js ahead of this file (EddieLib).
//
// Protocol (main thread -> worker):
//   load  {model}
//   plan  {requestId, question, site}
//   ask   {requestId, question, site, evidence: [{title, url, text}]}
//   abort {requestId?}
// (worker -> main thread):
//   progress    {text, progress}
//   loaded      {model, loadMs}
//   plan_result {requestId, queries, ms}
//   token       {requestId, text}
//   done        {requestId, answer, citations: [{n, url, title}], nohit, usage}
//   aborted     {requestId}
//   error       {requestId?, message}

"use strict";

const WEBLLM_URL = "https://esm.run/@mlc-ai/web-llm@0.2.84";
const EVIDENCE_CHARS = 700;

const lib = EddieLib;

let webllm = null;
let engine = null;
let modelId = null;
let loading = null;
let active = null; // { requestId, aborted }
let queue = Promise.resolve();

self.onmessage = function (e) {
  const msg = e.data || {};
  switch (msg.type) {
    case "load":
      load(msg);
      break;
    case "plan":
      enqueue(() => plan(msg), msg.requestId);
      break;
    case "ask":
      enqueue(() => ask(msg), msg.requestId);
      break;
    case "abort":
      abort(msg);
      break;
    default:
      postError(msg.requestId, `unknown message type ${String(msg.type)}`);
  }
};

function enqueue(fn, requestId) {
  queue = queue
    .then(() => {
      console.debug("eddie agent worker: start", requestId);
      return fn();
    })
    .catch((err) => postError(requestId, describe(err)))
    .then(() => console.debug("eddie agent worker: end", requestId));
}

async function load(msg) {
  const model = String(msg.model || "");
  if (!model) {
    postError(undefined, "load: model is required");
    return;
  }
  if (engine && modelId === model) {
    self.postMessage({ type: "loaded", model, loadMs: 0, cached: true });
    return;
  }
  if (loading) {
    try {
      await loading;
    } catch (_) {
      // fall through and try again
    }
    if (engine && modelId === model) {
      self.postMessage({ type: "loaded", model, loadMs: 0, cached: true });
      return;
    }
  }
  loading = (async () => {
    const t0 = performance.now();
    if (!webllm) {
      self.postMessage({ type: "progress", text: "Loading the WebLLM runtime…", progress: 0 });
      webllm = await import(WEBLLM_URL);
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
        self.postMessage({
          type: "progress",
          text: p && p.text ? p.text : "Loading model…",
          progress: p && typeof p.progress === "number" ? p.progress : null,
        });
      },
    });
    engine = created;
    modelId = model;
    self.postMessage({ type: "loaded", model, loadMs: Math.round(performance.now() - t0) });
  })();
  try {
    await loading;
  } catch (err) {
    engine = null;
    modelId = null;
    postError(undefined, describe(err));
  } finally {
    loading = null;
  }
}

function requireEngine() {
  if (!engine) throw new Error("model not loaded");
}

async function plan(msg) {
  requireEngine();
  const question = String(msg.question || "").trim();
  const site = String(msg.site || "this website");
  const t0 = performance.now();
  const reply = await engine.chat.completions.create({
    messages: [
      { role: "system", content: lib.planPrompt(site) },
      { role: "user", content: question },
    ],
    temperature: 0,
    max_tokens: 100,
    response_format: { type: "json_object", schema: JSON.stringify(lib.PLAN_SCHEMA) },
    extra_body: { enable_thinking: false },
  });
  const content = reply && reply.choices && reply.choices[0] && reply.choices[0].message ? reply.choices[0].message.content : "";
  const queries = lib.parsePlan(content, question);
  self.postMessage({ type: "plan_result", requestId: msg.requestId, queries, ms: Math.round(performance.now() - t0) });
}

async function ask(msg) {
  requireEngine();
  const requestId = msg.requestId;
  const question = String(msg.question || "").trim();
  const site = String(msg.site || "this website");
  const evidence = Array.isArray(msg.evidence) ? msg.evidence.filter((e) => e && e.url) : [];
  if (evidence.length === 0) {
    self.postMessage({
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
  active = { requestId, aborted: false };
  const t0 = performance.now();
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
        if (!first) first = performance.now();
        text += delta;
        self.postMessage({ type: "token", requestId, text: delta });
      }
      if (chunk && chunk.usage) usage = chunk.usage;
    }
  } finally {
    const wasAborted = active && active.aborted;
    active = null;
    if (wasAborted) {
      self.postMessage({ type: "aborted", requestId });
      return;
    }
  }
  const processed = lib.postProcessAnswer(text, evidence);
  const totalMs = Math.round(performance.now() - t0);
  self.postMessage({
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
  console.debug("eddie agent worker: abort", msg.requestId, active ? active.requestId : null);
  if (!active) return;
  if (msg.requestId != null && msg.requestId !== active.requestId) return;
  active.aborted = true;
  try {
    if (engine) engine.interruptGenerate();
  } catch (err) {
    console.warn("eddie agent: interrupt failed", err);
  }
}

function postError(requestId, message) {
  self.postMessage({ type: "error", requestId: requestId == null ? undefined : requestId, message });
}

function describe(err) {
  if (err == null) return "unknown error";
  if (typeof err === "string") return err;
  return err.message || String(err);
}
