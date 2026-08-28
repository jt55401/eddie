// SPDX-License-Identifier: GPL-3.0-only
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const D = require("../src/lib/download.js");

function streamOf(chunks) {
  let i = 0;
  return new ReadableStream({
    pull(controller) {
      if (i < chunks.length) controller.enqueue(chunks[i++]);
      else controller.close();
    },
  });
}

function response(chunks, headers, status) {
  return new Response(streamOf(chunks), { status: status || 200, headers: headers || {} });
}

const noSleep = async () => {};

test("streams with determinate progress when Content-Length is present", async () => {
  const calls = [];
  const bytes = await D.fetchBytes("https://x/model", {
    fetch: async () => response([new Uint8Array([1, 2, 3]), new Uint8Array([4, 5])], { "Content-Length": "5" }),
    onProgress: (loaded, total) => calls.push([loaded, total]),
    sleep: noSleep,
  });
  assert.deepEqual(Array.from(bytes), [1, 2, 3, 4, 5]);
  assert.deepEqual(calls, [[0, 5], [3, 5], [5, 5]]);
});

test("indeterminate progress without Content-Length", async () => {
  const calls = [];
  const bytes = await D.fetchBytes("https://x/model", {
    fetch: async () => response([new Uint8Array([9]), new Uint8Array([8, 7])]),
    onProgress: (loaded, total) => calls.push([loaded, total]),
    sleep: noSleep,
  });
  assert.deepEqual(Array.from(bytes), [9, 8, 7]);
  assert.deepEqual(calls, [[0, null], [1, null], [3, null]]);
});

test("retries once on a network error, then succeeds", async () => {
  let n = 0;
  const slept = [];
  const bytes = await D.fetchBytes("https://x/f", {
    fetch: async () => {
      n++;
      if (n === 1) throw new TypeError("Failed to fetch");
      return response([new Uint8Array([1])], { "Content-Length": "1" });
    },
    sleep: async (ms) => slept.push(ms),
    backoffMs: 10,
  });
  assert.equal(n, 2);
  assert.deepEqual(slept, [10]);
  assert.deepEqual(Array.from(bytes), [1]);
});

test("gives up after the retry", async () => {
  let n = 0;
  await assert.rejects(
    D.fetchBytes("https://x/f", { fetch: async () => { n++; throw new TypeError("net"); }, sleep: noSleep }),
    /net/
  );
  assert.equal(n, 2);
});

test("4xx is final, 5xx retries", async () => {
  let n = 0;
  await assert.rejects(
    D.fetchBytes("https://x/f", { fetch: async () => { n++; return response([], {}, 404); }, sleep: noSleep }),
    (e) => e instanceof D.HttpError && e.status === 404
  );
  assert.equal(n, 1);
  n = 0;
  await assert.rejects(
    D.fetchBytes("https://x/f", { fetch: async () => { n++; return response([], {}, 503); }, sleep: noSleep }),
    (e) => e.status === 503
  );
  assert.equal(n, 2);
});

test("truncated body with Content-Length is a retryable failure", async () => {
  let n = 0;
  await assert.rejects(
    D.fetchBytes("https://x/f", {
      fetch: async () => { n++; return response([new Uint8Array([1])], { "Content-Length": "3" }); },
      sleep: noSleep,
    }),
    /incomplete/
  );
  assert.equal(n, 2);
});

test("per-attempt timeout aborts the fetch and retries", async () => {
  let n = 0;
  const bytes = await D.fetchBytes("https://x/f", {
    timeoutMs: 5,
    fetch: (url, init) => new Promise((resolve, reject) => {
      n++;
      if (n === 1) {
        init.signal.addEventListener("abort", () => reject(Object.assign(new Error("aborted"), { name: "AbortError" })));
      } else {
        resolve(response([new Uint8Array([2])], { "Content-Length": "1" }));
      }
    }),
    sleep: noSleep,
  });
  assert.equal(n, 2);
  assert.deepEqual(Array.from(bytes), [2]);
});

test("sha256 helpers", async () => {
  const abc = new TextEncoder().encode("abc");
  const hex = await D.sha256Hex(abc);
  assert.equal(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  assert.equal(await D.verifySha256(abc, hex.toUpperCase()), true);
  assert.equal(await D.verifySha256(abc, "sha256:" + hex), true);
  assert.equal(await D.verifySha256(abc, "00".repeat(32)), false);
  assert.equal(await D.verifySha256(abc, "nothex"), false);
  assert.equal(await D.verifySha256(abc, ""), false);
});
