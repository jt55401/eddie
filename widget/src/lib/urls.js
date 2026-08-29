// SPDX-License-Identifier: GPL-3.0-only

// URL, version and cache-key helpers shared by the widget and the worker.

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

  const HF_HOST = "https://huggingface.co";

  /** Directory of an absolute URL, without query or hash, with a trailing slash. */
  function baseUrlOf(href) {
    const u = new URL(href);
    u.search = "";
    u.hash = "";
    const s = u.href;
    return s.substring(0, s.lastIndexOf("/") + 1);
  }

  /** The `v` query parameter of a URL (absolute or relative), or null. */
  function versionOf(href) {
    if (!href) return null;
    const q = href.indexOf("?");
    if (q < 0) return null;
    const params = new URLSearchParams(href.substring(q + 1).split("#")[0]);
    const v = params.get("v");
    return v ? v : null;
  }

  /** Append `?v=<version>` unless the URL already carries a `v` parameter. */
  function withVersion(url, version) {
    if (!version || versionOf(url)) return url;
    const hashAt = url.indexOf("#");
    const hash = hashAt >= 0 ? url.substring(hashAt) : "";
    const body = hashAt >= 0 ? url.substring(0, hashAt) : url;
    const sep = body.includes("?") ? (body.endsWith("?") || body.endsWith("&") ? "" : "&") : "?";
    return body + sep + "v=" + encodeURIComponent(version) + hash;
  }

  /** Runtime asset next to the widget script, version-busted when a version is known. */
  function assetUrl(baseUrl, name, version) {
    const base = baseUrl ? baseUrl.replace(/\/?$/, "/") : "";
    return withVersion(base + name, version);
  }

  /** `https://huggingface.co/<repo>/resolve/<revision>/<file>` */
  function hfFileUrl(repo, revision, file) {
    const rev = encodeURIComponent(revision || "main");
    const path = String(file).split("/").map(encodeURIComponent).join("/");
    return `${HF_HOST}/${repo}/resolve/${rev}/${path}`;
  }

  /** IndexedDB key for a cached model file. */
  function cacheKey(repo, revision, file) {
    return `${repo}@${revision || "main"}/${file}`;
  }

  const HF_RESOLVE_RE = /^https:\/\/huggingface\.co\/([^/]+\/[^/]+)\/resolve\/([^/]+)\/([^?#]+)/;

  /** The `cacheKey` of an `hfFileUrl`-shaped URL, or null for any other URL. */
  function cacheKeyFromUrl(url) {
    const m = HF_RESOLVE_RE.exec(String(url || ""));
    if (!m) return null;
    return cacheKey(m[1], decodeURIComponent(m[2]), m[3].split("/").map(decodeURIComponent).join("/"));
  }

  function isWeightsFile(file) {
    return /\.(safetensors|onnx|onnx_data|bin|data|pt|gguf)$/i.test(String(file));
  }

  /** Per-file fetch timeout: 10 minutes for weights, 60 seconds for everything else. */
  function timeoutForFile(file) {
    return isWeightsFile(file) ? 600000 : 60000;
  }

  return {
    HF_HOST,
    baseUrlOf,
    versionOf,
    withVersion,
    assetUrl,
    hfFileUrl,
    cacheKey,
    cacheKeyFromUrl,
    isWeightsFile,
    timeoutForFile,
  };
});
