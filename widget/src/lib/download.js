// SPDX-License-Identifier: GPL-3.0-only

// Streaming download with progress, per-attempt timeout and one retry, plus
// SHA-256 verification. `fetch`, `sleep` and `subtle` are injectable for tests.

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

  class HttpError extends Error {
    constructor(url, status) {
      super(`HTTP ${status} fetching ${url}`);
      this.name = "HttpError";
      this.status = status;
      this.url = url;
    }
  }

  function isRetryableStatus(status) {
    return status === 429 || status === 408 || status >= 500;
  }

  function isNetworkError(err) {
    if (!err) return false;
    if (err.name === "AbortError" || err.name === "TimeoutError") return true;
    if (err instanceof HttpError) return isRetryableStatus(err.status);
    // fetch() rejects with a TypeError on network failures (ERR_NETWORK_CHANGED etc.)
    return err.name === "TypeError";
  }

  async function readBody(response, onProgress) {
    const lenHeader = response.headers && response.headers.get ? response.headers.get("Content-Length") : null;
    const total = lenHeader && /^\d+$/.test(lenHeader) && Number(lenHeader) > 0 ? Number(lenHeader) : null;
    if (onProgress) onProgress(0, total);
    if (!response.body || typeof response.body.getReader !== "function") {
      const buf = new Uint8Array(await response.arrayBuffer());
      if (onProgress) onProgress(buf.length, total == null ? buf.length : total);
      return buf;
    }
    const reader = response.body.getReader();
    let loaded = 0;
    if (total != null) {
      const out = new Uint8Array(total);
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (loaded + value.length > total) {
          throw new Error("response longer than Content-Length");
        }
        out.set(value, loaded);
        loaded += value.length;
        if (onProgress) onProgress(loaded, total);
      }
      if (loaded !== total) {
        throw new TypeError(`incomplete response: ${loaded} of ${total} bytes`);
      }
      return out;
    }
    const chunks = [];
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      loaded += value.length;
      if (onProgress) onProgress(loaded, null);
    }
    const out = new Uint8Array(loaded);
    let offset = 0;
    for (const c of chunks) {
      out.set(c, offset);
      offset += c.length;
    }
    return out;
  }

  /**
   * Download `url` as a Uint8Array.
   * opts: { fetch, timeoutMs (per attempt), retries (default 1), backoffMs (default 1500),
   *         onProgress(loaded, total|null), sleep, signal }
   * Retries once on network errors, timeouts, 408/429/5xx. 4xx is final.
   */
  async function fetchBytes(url, opts) {
    const o = opts || {};
    const doFetch = o.fetch || globalThis.fetch;
    const sleep = o.sleep || ((ms) => new Promise((r) => setTimeout(r, ms)));
    const retries = o.retries == null ? 1 : o.retries;
    const backoffMs = o.backoffMs == null ? 1500 : o.backoffMs;
    let lastErr = null;
    for (let attempt = 0; attempt <= retries; attempt++) {
      const controller = typeof AbortController === "function" ? new AbortController() : null;
      let timer = null;
      if (controller && o.timeoutMs) {
        timer = setTimeout(() => controller.abort(), o.timeoutMs);
      }
      const onOuterAbort = () => controller && controller.abort();
      if (o.signal && controller) o.signal.addEventListener("abort", onOuterAbort);
      try {
        const response = await doFetch(url, { signal: controller ? controller.signal : undefined, cache: "default" });
        if (!response.ok) {
          throw new HttpError(url, response.status);
        }
        return await readBody(response, o.onProgress);
      } catch (err) {
        lastErr = err;
        if (o.signal && o.signal.aborted) throw err;
        if (attempt < retries && isNetworkError(err)) {
          await sleep(backoffMs * (attempt + 1));
          continue;
        }
        throw err;
      } finally {
        if (timer) clearTimeout(timer);
        if (o.signal && controller) o.signal.removeEventListener("abort", onOuterAbort);
      }
    }
    throw lastErr;
  }

  function toHex(buffer) {
    const bytes = new Uint8Array(buffer);
    let s = "";
    for (let i = 0; i < bytes.length; i++) {
      s += bytes[i].toString(16).padStart(2, "0");
    }
    return s;
  }

  async function sha256Hex(bytes, subtle) {
    const s = subtle || (globalThis.crypto && globalThis.crypto.subtle);
    if (!s) throw new Error("crypto.subtle unavailable");
    const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    return toHex(await s.digest("SHA-256", view));
  }

  /** True when the SHA-256 of `bytes` equals `expectedHex` (case-insensitive, optional "sha256:" prefix). */
  async function verifySha256(bytes, expectedHex, subtle) {
    if (!expectedHex) return false;
    const want = String(expectedHex).replace(/^sha256:/i, "").trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(want)) return false;
    const got = await sha256Hex(bytes, subtle);
    return got === want;
  }

  return { HttpError, isNetworkError, isRetryableStatus, fetchBytes, sha256Hex, verifySha256, toHex };
});
