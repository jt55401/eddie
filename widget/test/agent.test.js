// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const A = Object.assign({}, require("../src/lib/agent.js"), require("../src/lib/agent-llm.js"));

const GIB = 1024 * 1024 * 1024;

test("model selection: auto picks 2B on big desktop adapters, 0.8B otherwise", () => {
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 4 * GIB, isMobile: false, hasF16: false }).id, "Qwen3.5-2B-q4f32_1-MLC");
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 4 * GIB, isMobile: false, hasF16: true }).id, "Qwen3.5-2B-q4f16_1-MLC");
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 1 * GIB, isMobile: false, hasF16: true }).id, "Qwen3.5-0.8B-q4f16_1-MLC");
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 4 * GIB, isMobile: true, hasF16: false }).id, "Qwen3.5-0.8B-q4f32_1-MLC");
  assert.equal(A.selectAgentModel({ mode: "quality", maxBufferSize: 256 * 1024 * 1024, isMobile: true }).id, "Qwen3.5-2B-q4f32_1-MLC");
  const explicit = A.selectAgentModel({ mode: "Qwen3.5-4B-q4f16_1-MLC", hasF16: false });
  assert.equal(explicit.id, "Qwen3.5-4B-q4f16_1-MLC");
  assert.equal(explicit.base, "Qwen3.5-4B");
  assert.equal(explicit.sizeBytes, 2.3e9);
  assert.equal(A.selectAgentModel({ mode: "gemma3-1b-it-q4f32_1-MLC" }).sizeBytes, null);
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 4 * GIB }).sizeBytes, 1.2e9);
});

test("mobile detection", () => {
  assert.equal(A.isMobileDevice({ userAgentData: { mobile: true } }), true);
  assert.equal(A.isMobileDevice({ userAgentData: { mobile: false }, userAgent: "iPhone" }), false);
  assert.equal(A.isMobileDevice({ userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)" }), true);
  assert.equal(A.isMobileDevice({ userAgent: "Mozilla/5.0 (X11; Linux x86_64) Chrome/151" }), false);
});

test("think stripping", () => {
  assert.equal(A.stripThink("<think>\nplan\n</think>\n\nHello [1]"), "Hello [1]");
  assert.equal(A.stripThink("<think></think>Hi"), "Hi");
  assert.equal(A.stripThink("<think>dangling answer"), "dangling answer");
  assert.equal(A.stripThink(""), "");
  assert.equal(A.visibleStreamText("<think>hmm"), "");
  assert.equal(A.visibleStreamText("<think>hmm</think>Yes"), "Yes");
  assert.equal(A.visibleStreamText("Yes <think>more"), "Yes ");
});

test("plan parsing: JSON, dedupe, limits, fallback", () => {
  assert.deepEqual(A.parsePlan('{"queries": ["rust tools", "Rust Tools", "common crawl", "x", "fourth"]}', "q?"), ["rust tools", "common crawl", "fourth"]);
  assert.deepEqual(A.parsePlan('Sure! ```json\n{"queries":["jason companies"]}\n```', "q?"), ["jason companies"]);
  assert.deepEqual(A.parsePlan("<think>x</think>{\"queries\":[\" no_think \", \"companies worked\"]}", "q?"), ["companies worked"]);
  assert.deepEqual(A.parsePlan("garbage", "which companies?"), ["which companies?"]);
  assert.deepEqual(A.parsePlan('{"queries": "not-an-array"}', "q"), ["q"]);
  assert.deepEqual(A.parsePlan('{"queries": [42, null]}', "q"), ["q"]);
});

test("evidence merge is round-robin and deduped by url", () => {
  const a = [{ url: "/a/" }, { url: "/b/" }, { url: "/c/" }];
  const b = [{ url: "/b" }, { url: "/d/" }];
  const c = [{ url: "/E/#x" }, { url: "/e/" }];
  assert.deepEqual(A.mergeEvidence([a, b, c], 6).map((r) => r.url), ["/a/", "/b", "/E/#x", "/b/", "/d/", "/c/"].filter((u) => u !== "/b/"));
  assert.deepEqual(A.mergeEvidence([a, b], 2).map((r) => r.url), ["/a/", "/b"]);
  assert.deepEqual(A.mergeEvidence([[], null], 3), []);
});

test("truncation and evidence formatting", () => {
  const long = "word ".repeat(300).trim();
  const t = A.truncateText(long, 700);
  assert.ok(t.length <= 700);
  assert.ok(t.endsWith("…"));
  assert.equal(A.truncateText("short  text\n here", 700), "short text here");
  const s = A.formatEvidence([{ title: "Bio", url: "/about/", text: "Jason is a technologist." }, { url: "/x/", snippet: "snip" }]);
  assert.equal(s, "[1] Bio (/about/)\nJason is a technologist.\n\n[2] /x/ (/x/)\nsnip");
  assert.match(A.sourcesPrompt([{ title: "T", url: "/t/", text: "x" }], "why?"), /^Sources:\n\n\[1\] T \(\/t\/\)\nx\n\nQuestion: why\?$/);
});

const ev = [
  { title: "Bio", url: "/about/", text: "a" },
  { title: "AI Resume", url: "/ai/", text: "b" },
  { title: "Checker", url: "/posts/cc/", text: "c" },
];

test("post-processing maps citations and drops out-of-range ones", () => {
  const r = A.postProcessAnswer("<think>\n</think>Jason worked at Warecorp [1] and Airborne [2][7]. Also see [3, 9].", ev);
  assert.equal(r.answer, "Jason worked at Warecorp [1] and Airborne [2]. Also see [3].");
  assert.deepEqual(r.citations, [
    { n: 1, url: "/about/", title: "Bio" },
    { n: 2, url: "/ai/", title: "AI Resume" },
    { n: 3, url: "/posts/cc/", title: "Checker" },
  ]);
  assert.equal(r.nohit, false);
});

test("post-processing drops a trailing fallback sentence when an answer exists", () => {
  const r = A.postProcessAnswer("The Common Crawl Checker is a web tool. [3]\nThe site doesn't cover that.", ev);
  assert.equal(r.answer, "The Common Crawl Checker is a web tool. [3]");
  assert.deepEqual(r.citations.map((c) => c.n), [3]);
  assert.equal(r.nohit, false);
});

test("post-processing marks a fallback-only reply as nohit", () => {
  for (const raw of ["The site doesn't cover that.", "<think></think>\nThe site does not cover that", "No, the site doesn't cover that. [1]", ""]) {
    const r = A.postProcessAnswer(raw, ev);
    assert.equal(r.nohit, true, raw);
    assert.equal(r.answer, A.NOHIT);
    assert.deepEqual(r.citations, []);
  }
});

test("post-processing merges stray citation lines and bold markers", () => {
  const r = A.postProcessAnswer("Yes, he writes Rust tools [1]. He has for years [1].\n\n**[1]**", ev);
  assert.equal(r.answer, "Yes, he writes Rust tools [1]. He has for years [1].");
  assert.deepEqual(r.citations.map((c) => c.n), [1]);
});

test("prompts carry the site name and the schema is well-formed", () => {
  assert.match(A.planPrompt("the personal website of Jason Grey"), /The site is the personal website of Jason Grey\./);
  assert.match(A.answerPrompt("example.com"), /about example\.com using only/);
  assert.equal(A.PLAN_SCHEMA.properties.queries.maxItems, 3);
  assert.equal(A.baseModelId("Qwen3.5-2B-q4f32_1-MLC"), "Qwen3.5-2B");
});

test("post-processing handles a dangling <think> and keeps citations after it", () => {
  const r = A.postProcessAnswer("<think>\nreasoning that never closed\nJason writes about Rust [1].", ev);
  assert.equal(r.answer, "reasoning that never closed\nJason writes about Rust [1].");
  assert.deepEqual(r.citations.map((c) => c.n), [1]);
});

test("post-processing with no evidence drops every citation", () => {
  const r = A.postProcessAnswer("Something [1][2].", []);
  assert.equal(r.answer, "Something.");
  assert.deepEqual(r.citations, []);
  assert.equal(r.nohit, false);
});

test("post-processing keeps sentence order and dedupes repeated citations", () => {
  const r = A.postProcessAnswer("A [2]. B [2] [2]. C [1, 2].", ev);
  assert.equal(r.answer, "A [2]. B [2]. C [1][2].");
  assert.deepEqual(r.citations.map((c) => c.n), [1, 2]);
});

test("model selection: the auto threshold is exactly 2 GiB", () => {
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 2 * GIB, isMobile: false }).base, "Qwen3.5-2B");
  assert.equal(A.selectAgentModel({ mode: "auto", maxBufferSize: 2 * GIB - 1, isMobile: false }).base, "Qwen3.5-0.8B");
  assert.equal(A.selectAgentModel({ mode: "auto" }).base, "Qwen3.5-0.8B");
  assert.equal(A.selectAgentModel({}).id, "Qwen3.5-0.8B-q4f32_1-MLC");
  assert.equal(A.agentModelBytes("Qwen3.5-0.8B-q4f16_1-MLC"), 0.4e9);
});

test("faqPasses prefers the confident flag and falls back to the score", () => {
  assert.equal(A.faqPasses({ score: 0.9, confident: false }, "auto"), false);
  assert.equal(A.faqPasses({ score: 0.3, confident: true }, "auto"), true);
  assert.equal(A.faqPasses({ score: 0.6 }, "auto"), true);
  assert.equal(A.faqPasses({ score: 0.4 }, "auto"), false);
  assert.equal(A.faqPasses({ score: 0.1 }, "always"), true);
  assert.equal(A.faqPasses({ score: 0.99, confident: true }, "off"), false);
  assert.equal(A.faqPasses(null, "auto"), false);
});

test("qaEvidence turns confident hits into Q/A evidence items", () => {
  const hits = [
    { question: "How long has Jason been coding?", answer: "Nearly 40 years.", source_url: "/skills/programming-languages/", confident: true },
    { question: "Years in AI/ML?", answer: "20+", source_url: "/r/", confident: false },
    { question: "First paid job?", answer: "Age 14.", source_url: "/skills/programming-languages/", score: 0.7 },
  ];
  const items = A.qaEvidence(hits, 2);
  assert.equal(items.length, 2);
  assert.equal(items[0].title, "FAQ: How long has Jason been coding?");
  assert.match(items[0].text, /^Q: .*\nA: Nearly 40 years\.$/);
  assert.equal(items[0].url, "/skills/programming-languages/");
  assert.equal(items[1].title, "FAQ: First paid job?");
});
