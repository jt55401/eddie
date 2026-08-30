// SPDX-License-Identifier: GPL-3.0-only

// Transports between the widget and its engines.
//
// DedicatedWorkerTransport wraps a Worker; ServiceWorkerTransport wraps one
// MessageChannel to the Eddie service worker (widget/src/eddie-sw.js). Both
// expose the same surface so the widget does not care which one it holds:
//
//   call(type, payload, opts?) -> Promise of the reply carrying the requestId
//   on(type, handler)          -> unsubscribe fn (unsolicited messages: status,
//                                 ready, progress, token, done, aborted, error,
//                                 reset, crash)
//   postRaw(message)              fire-and-forget (abort, ask)
//   nextId()                      request id for postRaw flows
//   terminate()
//
// The pure decision helpers at the bottom (transport kind, keepalive, state
// reuse) are what the node tests cover.

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

  const HELLO_TIMEOUT_MS = 3000;
  // Liveness checks must tolerate a busy worker: model loading blocks its
  // thread for seconds at a time (ONNX session creation, safetensors parsing).
  const PING_TIMEOUT_MS = 10000;
  const IDLE_PING_TIMEOUT_MS = 2000;
  const RECONNECT_HELLO_TIMEOUT_MS = 10000;
  const KEEPALIVE_MS = 15000;
  const REGISTER_TIMEOUT_MS = 20000;

  class Emitter {
    constructor() {
      this.handlers = new Map();
    }
    on(type, handler) {
      if (!this.handlers.has(type)) this.handlers.set(type, new Set());
      this.handlers.get(type).add(handler);
      return () => this.off(type, handler);
    }
    off(type, handler) {
      const set = this.handlers.get(type);
      if (set) set.delete(handler);
    }
    emit(type, msg) {
      const set = this.handlers.get(type);
      if (!set) return false;
      for (const h of Array.from(set)) {
        try {
          h(msg);
        } catch (err) {
          console.warn("eddie transport handler failed", err);
        }
      }
      return set.size > 0;
    }
  }

  function replyError(msg) {
    const err = new Error(msg.message || "request failed");
    err.fatal = !!msg.fatal;
    err.unsupported = !!msg.unsupported;
    return err;
  }

  /** Request/reply bookkeeping shared by both transports. */
  class BaseTransport extends Emitter {
    constructor(env) {
      super();
      this.env = env || {};
      this.pending = new Map(); // requestId -> { resolve, reject, timer }
      this.seq = 0;
      this.closed = false;
    }
    /** Ids for the transport's own requests (hello, ping, state); the widget numbers its own. */
    nextId() {
      return "t" + ++this.seq;
    }
    call(type, payload, opts) {
      const o = opts || {};
      return new Promise((resolve, reject) => {
        if (this.closed) {
          reject(new Error("transport closed"));
          return;
        }
        const requestId = o.requestId != null ? o.requestId : this.nextId();
        const entry = { resolve, reject, timer: null };
        if (o.timeoutMs) {
          const st = this.env.setTimeout || setTimeout;
          entry.timer = st(() => {
            if (this.pending.get(requestId) !== entry) return;
            this.pending.delete(requestId);
            const err = new Error(`${type}: no reply within ${o.timeoutMs} ms`);
            err.timeout = true;
            reject(err);
          }, o.timeoutMs);
        }
        this.pending.set(requestId, entry);
        try {
          this.postRaw(Object.assign({ type, requestId }, payload || {}));
        } catch (err) {
          this.pending.delete(requestId);
          reject(err);
        }
      });
    }
    failAllPending(err) {
      const entries = Array.from(this.pending.values());
      this.pending.clear();
      for (const p of entries) {
        if (p.timer) (this.env.clearTimeout || clearTimeout)(p.timer);
        p.reject(err);
      }
    }
    handleMessage(msg) {
      const m = msg || {};
      if (m.requestId != null && this.pending.has(m.requestId)) {
        const p = this.pending.get(m.requestId);
        // Streaming replies (token) share the requestId with the final
        // `done`; only terminal messages settle the promise.
        if (m.type === "token" || m.type === "progress") {
          this.emit(m.type, m);
          return;
        }
        this.pending.delete(m.requestId);
        if (p.timer) (this.env.clearTimeout || clearTimeout)(p.timer);
        if (m.type === "error") {
          p.reject(replyError(m));
          if (m.fatal) this.emit("error", m);
        } else {
          p.resolve(m);
        }
        return;
      }
      this.emit(m.type, m);
    }
  }

  /** A dedicated Worker (classic or module). */
  class DedicatedWorkerTransport extends BaseTransport {
    constructor(url, opts) {
      super(opts && opts.env);
      const o = opts || {};
      const WorkerCtor = o.Worker || globalThis.Worker;
      this.kind = "worker";
      this.worker = o.type ? new WorkerCtor(url, { type: o.type }) : new WorkerCtor(url);
      this.worker.onmessage = (e) => this.handleMessage(e.data || {});
      this.worker.onerror = (e) => {
        const message = e && e.message ? e.message : "worker failed to load";
        const err = new Error(message);
        this.failAllPending(err);
        this.emit("crash", { type: "crash", message });
      };
    }
    get persistent() {
      return false;
    }
    postRaw(msg) {
      this.worker.postMessage(msg);
    }
    ping() {
      return Promise.resolve({ type: "pong" });
    }
    setKeepalive() {
      // a dedicated worker lives as long as the page
    }
    terminate() {
      this.closed = true;
      this.failAllPending(new Error("worker terminated"));
      this.worker.terminate();
    }
  }

  /**
   * One MessageChannel to the Eddie service worker. `connect()` transfers a
   * port with a `connect` message and waits for `hello`; the keepalive pings
   * catch a service worker Chrome stopped while idle (ports die with it) and
   * reconnect, emitting `reset` so the widget re-runs its init flow.
   */
  class ServiceWorkerTransport extends BaseTransport {
    constructor(registration, opts) {
      super(opts && opts.env);
      const o = opts || {};
      this.kind = "sw";
      this.registration = registration;
      this.channelKind = o.kind || "search";
      this.version = o.version || null;
      this.helloTimeoutMs = o.helloTimeoutMs || HELLO_TIMEOUT_MS;
      this.pingTimeoutMs = o.pingTimeoutMs || PING_TIMEOUT_MS;
      this.keepaliveMs = o.keepaliveMs || KEEPALIVE_MS;
      this.MessageChannel = o.MessageChannel || globalThis.MessageChannel;
      this.port = null;
      this.hello = null;
      this.keepaliveTimer = null;
      this.reconnecting = null;
    }
    get persistent() {
      return true;
    }
    activeWorker() {
      const r = this.registration;
      return r.active || r.waiting || r.installing || null;
    }
    async connect() {
      const sw = this.activeWorker();
      if (!sw) throw new Error("service worker not active");
      if (this.port) {
        try {
          this.port.close();
        } catch (_) {
          // ignore
        }
      }
      const ch = new this.MessageChannel();
      ch.port1.onmessage = (e) => this.handleMessage(e.data || {});
      this.port = ch.port1;
      sw.postMessage({ type: "connect", kind: this.channelKind, version: this.version }, [ch.port2]);
      this.hello = await this.call("hello", {}, { timeoutMs: this.helloTimeoutMs });
      return this.hello;
    }
    postRaw(msg) {
      if (!this.port) throw new Error("service worker transport not connected");
      this.port.postMessage(msg);
    }
    ping(timeoutMs) {
      return this.call("ping", {}, { timeoutMs: timeoutMs || this.pingTimeoutMs });
    }
    state() {
      return this.call("state", {}, { timeoutMs: this.pingTimeoutMs });
    }
    /** Re-establish the channel after the service worker was stopped. */
    reconnect() {
      if (this.reconnecting) return this.reconnecting;
      this.reconnecting = (async () => {
        this.failAllPending(new Error("service worker restarted"));
        const saved = this.helloTimeoutMs;
        this.helloTimeoutMs = Math.max(saved, RECONNECT_HELLO_TIMEOUT_MS);
        let hello;
        try {
          hello = await this.connect();
        } finally {
          this.helloTimeoutMs = saved;
        }
        this.emit("reset", { type: "reset", hello });
        return hello;
      })().finally(() => {
        this.reconnecting = null;
      });
      return this.reconnecting;
    }
    /**
     * Ping; on silence reconnect. Resolves true when the channel is usable.
     * `timeoutMs` may be short when the worker is known to be idle (a live
     * idle worker answers within milliseconds; a stopped one never does),
     * and must be generous while it may be loading a model.
     */
    async ensureAlive(timeoutMs) {
      try {
        await this.ping(timeoutMs);
        return true;
      } catch (err) {
        if (this.closed) return false;
        console.debug(`eddie: ${this.channelKind} channel silent (${err && err.message}); reconnecting`);
        try {
          await this.reconnect();
          console.debug(`eddie: ${this.channelKind} channel reconnected`);
          return true;
        } catch (err2) {
          console.warn("eddie: service worker reconnect failed", err2);
          this.emit("crash", { type: "crash", message: err2 && err2.message ? err2.message : String(err2) });
          return false;
        }
      }
    }
    /**
     * Keepalive pings are fire-and-forget: their only job is to keep Chrome
     * from stopping the worker as idle. Liveness is checked explicitly
     * (ensureAlive) at the points where a dead worker would otherwise hang
     * the widget, so a worker that is merely busy is never mistaken for a
     * stopped one.
     */
    setKeepalive(wanted) {
      const si = this.env.setInterval || setInterval;
      const ci = this.env.clearInterval || clearInterval;
      if (wanted && !this.keepaliveTimer) {
        this.keepaliveTimer = si(() => {
          try {
            this.postRaw({ type: "ping", requestId: this.nextId() });
          } catch (_) {
            // not connected; ensureAlive will deal with it
          }
        }, this.keepaliveMs);
      } else if (!wanted && this.keepaliveTimer) {
        ci(this.keepaliveTimer);
        this.keepaliveTimer = null;
      }
    }
    terminate() {
      this.closed = true;
      this.setKeepalive(false);
      this.failAllPending(new Error("transport closed"));
      if (this.port) {
        try {
          this.port.close();
        } catch (_) {
          // ignore
        }
        this.port = null;
      }
    }
  }

  /**
   * Register the Eddie service worker and wait until a worker is active.
   * Never use `navigator.serviceWorker.ready` here: it only resolves for the
   * registration controlling the *page*, and pages live outside the asset
   * directory that scopes this worker.
   */
  function registerServiceWorker(opts) {
    const o = opts || {};
    const container = o.container;
    const st = o.setTimeout || setTimeout;
    const ct = o.clearTimeout || clearTimeout;
    return new Promise((resolve, reject) => {
      let done = false;
      const timer = st(() => {
        if (done) return;
        done = true;
        reject(new Error("service worker registration timed out"));
      }, o.timeoutMs || REGISTER_TIMEOUT_MS);
      const finish = (fn) => {
        if (done) return;
        done = true;
        ct(timer);
        fn();
      };
      Promise.resolve()
        .then(() => container.register(o.url, { type: "module", scope: o.scope, updateViaCache: "none" }))
        .then((registration) => {
          if (registration.active) {
            finish(() => resolve(registration));
            return;
          }
          const sw = registration.installing || registration.waiting;
          if (!sw) {
            finish(() => reject(new Error("service worker registration has no worker")));
            return;
          }
          const onChange = () => {
            if (sw.state === "activated") {
              sw.removeEventListener("statechange", onChange);
              finish(() => resolve(registration));
            } else if (sw.state === "redundant") {
              sw.removeEventListener("statechange", onChange);
              finish(() => reject(new Error("service worker became redundant during install")));
            }
          };
          sw.addEventListener("statechange", onChange);
          onChange();
        })
        .catch((err) => finish(() => reject(err)));
    });
  }

  // -- pure decisions -----------------------------------------------------

  /**
   * "sw" when persistence is on and the browser can host it, else "worker".
   * Service workers need a secure context (localhost counts).
   */
  function chooseTransportKind(opts) {
    const o = opts || {};
    if (o.persist === "off") return "worker";
    if (!o.hasServiceWorker) return "worker";
    if (o.secureContext === false) return "worker";
    return "sw";
  }

  /** Keep the service worker alive while the visitor is using it. */
  function keepaliveWanted(opts) {
    const o = opts || {};
    return !!(o.open || o.streaming || o.pending);
  }

  /** The service worker already holds the model this page would load. */
  function canReuseAgent(state, modelId) {
    return !!(state && state.loaded && modelId && state.model === modelId);
  }

  /** The service worker already holds a ready engine for this exact index URL. */
  function canReuseSearch(state, indexUrl) {
    return !!(state && state.phase === "ready" && state.indexLoaded && indexUrl && state.indexUrl === indexUrl);
  }

  /**
   * Whether search should stay in a page-side worker even though a service
   * worker is available: the service worker cannot run webgpu-onnx lanes
   * (`swOnnx` false, no WebGPU in its scope) while this page has an adapter
   * and the site allows the WebGPU runtime. Persistence is not worth a
   * quieter dense lane. Only the gpu tier can run those lanes at all; the
   * lite and dense tiers never hold a webgpu lane, so the question does not
   * arise for them.
   */
  function searchStaysOnPage(opts) {
    const o = opts || {};
    if (o.tier && o.tier !== "gpu") return false;
    if (o.swOnnx) return false;
    if (!o.pageHasGpu) return false;
    return o.denseRuntime !== "wasm";
  }

  // -- service worker tiers ------------------------------------------------
  //
  // Four builds of widget/src/eddie-sw.js (see widget/build.sh), each in
  // its own scope under the asset directory, so a page installs only the
  // imports its visitor has opted into:
  //   lite   eddie-sw-lite.js   keyword + sparse search (lite wasm)
  //   dense  eddie-sw-dense.js  + the CPU dense lane (dense wasm)
  //   gpu    eddie-sw-gpu.js    + transformers.js (WebGPU search lane)
  //   agent  eddie-sw-agent.js  WebLLM + the agent engine, registered at
  //                             agent consent; never a search host

  // The search tiers. The agent tier is separate: no index lane maps to it
  // and it is never remembered as a search tier.
  const SW_TIERS = ["lite", "dense", "gpu"];

  function swScriptName(tier) {
    return `eddie-sw-${tier}.js`;
  }

  /** Registration scope of a tier: `<asset dir>sw/<tier>/` (a key, never navigated to). */
  function swScope(baseUrl, tier) {
    return String(baseUrl).replace(/\/?$/, "/") + "sw/" + tier + "/";
  }

  /** The tier that can host a dense lane of `kind` ("wasm-candle" | "webgpu-onnx" | null). */
  function tierForLane(kind) {
    if (kind === "webgpu-onnx") return "gpu";
    if (kind === "wasm-candle") return "dense";
    return "lite";
  }

  /**
   * Which tier the search engine should live in right now: the tier of the
   * lane about to be loaded (`laneKind`, known once the engine asked for
   * consent or the visitor accepted), else the tier remembered from an
   * earlier consent on this browser (`rememberedTier`), else lite.
   */
  function searchTierFor(opts) {
    const o = opts || {};
    if (o.laneKind) return tierForLane(o.laneKind);
    if (SW_TIERS.includes(o.rememberedTier)) return o.rememberedTier;
    return "lite";
  }

  /**
   * Eddie 0.4.2 registered one service worker with scope = the asset
   * directory. Its script (eddie-sw.js) no longer ships; unregister it so
   * the browser stops trying to update it. Any other registration matching
   * that URL (the site's own worker at "/") is left alone.
   */
  async function unregisterLegacyServiceWorker(container, baseUrl) {
    if (!container || typeof container.getRegistration !== "function") return false;
    let reg;
    try {
      reg = await container.getRegistration(baseUrl);
    } catch (_) {
      return false;
    }
    if (!reg || reg.scope !== baseUrl) return false;
    try {
      await reg.unregister();
      return true;
    } catch (_) {
      return false;
    }
  }

  return {
    SW_TIERS,
    swScriptName,
    swScope,
    tierForLane,
    searchTierFor,
    unregisterLegacyServiceWorker,
    HELLO_TIMEOUT_MS,
    PING_TIMEOUT_MS,
    IDLE_PING_TIMEOUT_MS,
    KEEPALIVE_MS,
    Emitter,
    BaseTransport,
    DedicatedWorkerTransport,
    ServiceWorkerTransport,
    registerServiceWorker,
    chooseTransportKind,
    keepaliveWanted,
    canReuseAgent,
    canReuseSearch,
    searchStaysOnPage,
  };
});
