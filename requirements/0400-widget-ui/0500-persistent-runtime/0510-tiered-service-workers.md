# 0510 Persistent Engines and Tiered Service Workers

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor who navigates between pages on the same site, I don't
pay again to re-initialize search or the agent on the second page, because
the engines can live in a service worker that survives page navigation;
where that isn't possible (or the site owner turns it off), search and the
agent still work, hosted in a page-side worker instead.

## Key Fields/Parameters

- `data-persist` (`auto` default | `off`): `auto` keeps the search engine and the agent in a module service worker when `navigator.serviceWorker` exists, the page is a secure context, and registration succeeds; `off` always uses page-side dedicated/module workers (`eddie-worker.js`, `eddie-agent-worker.js`)
- `data-warm` (`auto` default | `off` | `always`): controls when the engine initializes relative to the modal opening — `auto` does nothing for a first-time visitor and initializes search after `load` (idle callback) for one who has searched or accepted a model before on this browser (`localStorage` `eddie.search.used` / `eddie.search.consent`), unless Data Saver or `prefers-reduced-data` is set; `always` also initializes for a first-time visitor (still not under Data Saver); `off` waits for the first search
- three service worker tiers, one build each of the single `widget/src/eddie-sw.js` source because a service worker cannot use dynamic `import()` and must carry its dependencies as static imports: lite (`eddie-sw-lite.js`, scope `sw/lite/`, imports `eddie-lite-esm.js`), dense (`eddie-sw-dense.js`, scope `sw/dense/`, lite plus `eddie-dense-esm.js`), gpu (`eddie-sw-gpu.js`, scope `sw/gpu/`, lite plus `eddie-transformers-sw.js` and WebLLM)
- registration is lazy and per tier: lite registers on the first modal open (or a returning visitor's warm-up); dense or gpu registers only when a lane of that kind is accepted (`switchSearchTier`: new `MessageChannel`, `init` with consent, the old worker left to idle out); the agent always uses the gpu tier; the chosen search tier is remembered in `localStorage` `eddie.search.tier` so the next page registers it directly
- transport selection (`widget/src/lib/transport.js`): `DedicatedWorkerTransport` and `ServiceWorkerTransport` share one `call`/`on`/`postRaw`/`terminate` surface; a service worker registration must answer `hello` within 3 s reporting the expected tier or the widget falls back to page-side workers for that page; a modal opened before the decision resolves waits at most 3 s
- state reuse: `hello`/`state` carry both engines' snapshots; a page whose index URL (`?v=` included) matches an already-`ready` engine adopts that state at once instead of sending `init` (`data-reused="true"` / `data-agent-reused="true"` on the host element); a loaded agent model with the chosen id is reused without a `load`
- keepalive: the widget pings the service worker every 15 s while the modal is open, an answer is streaming, or a request is pending; a ping without `pong` within 5 s reconnects (new channel, new `hello`) and emits `reset`, which re-runs `init` if the modal is open and reloads the model on the next Ask
- fallback triggers, all silent and all resulting in page-side workers that speak the identical protocol: no `navigator.serviceWorker`, an insecure context, `register()` rejecting (e.g. a blocked CDN import for the gpu tier), no `hello` within the timeout, or `data-persist="off"`

## Acceptance Criteria

- With `data-persist="auto"` and service workers supported, opening search on a second same-site page whose index URL matches an already-`ready` service worker engine reuses that engine (`data-reused="true"`) instead of re-fetching or re-parsing the index or reloading a dense/agent model.
- With `data-persist="off"`, or on a browser/context where service workers are unavailable or fail to register, search and the agent behave identically to the service-worker path, just hosted in page-side dedicated/module workers.
- A visitor who never opens search causes no service worker of any tier to register; a plain page view registers nothing.
- Accepting a `wasm-candle` or `webgpu-onnx` search lane registers or upgrades only the service worker tier that lane needs; accepting the agent independently ensures the gpu tier without requiring a `webgpu-onnx` search lane to have been accepted first.
- A stopped or idled service worker is detected on the next interaction and transparently reconnected; a request in flight when this happens is retried once rather than surfaced to the visitor as an error.
- `data-warm="auto"` performs no network activity for a first-time visitor's plain page view, and never initializes a model download on its own — only `init`/`cache_check` for a lane already consented to and cached; `data-warm="always"` may also download an uncached lane's files without an explicit search, still never under Data Saver.

## Evidence

- `widget/README.md` — [Persistent engines](../../../widget/README.md#persistent-engines), [Loader](../../../widget/README.md#loader)
- `widget/build.sh`
- `widget/src/lib/transport.js`, `widget/test/transport.test.js`
- `widget/src/lib/boot.js`, `widget/test/boot.test.js`
- `widget/src/lib/warm.js`, `widget/test/warm.test.js`
- `widget/src/eddie-sw.js`

## Linked Tickets

- (none yet)
