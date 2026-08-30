// SPDX-License-Identifier: GPL-3.0-only

// Eddie agent worker (module worker, created on the first "Ask"): the
// fallback host when the service worker (eddie-sw.js) is unavailable, same
// protocol either way. The engine is widget/src/lib/agent-engine.js; this
// file binds it to a dedicated module worker and loads WebLLM with a dynamic
// import(). widget/build.sh concatenates widget/src/lib/agent.js and
// agent-engine.js ahead of this file (EddieLib).
//
// Protocol: see widget/README.md ("Worker protocol", agent section).

"use strict";

const WEBLLM_URL = "https://esm.run/@mlc-ai/web-llm@0.2.84";

const lib = EddieLib;

const engine = lib.createAgentEngine({
  post: (message) => self.postMessage(message),
  loadWebLLM: () => import(WEBLLM_URL),
});

self.onmessage = function (e) {
  engine.handle(e.data || {}, (message) => self.postMessage(message));
};
