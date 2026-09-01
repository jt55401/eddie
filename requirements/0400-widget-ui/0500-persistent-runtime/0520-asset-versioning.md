# 0520 Runtime Asset Versioning and Caching

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor who comes back after the site has published new content,
I don't re-download the search engine, because the runtime assets are
versioned by what built them and only change when the site upgrades Eddie
itself; and as a site owner I can serve the whole asset directory with a
one-year `immutable` cache without ever shipping a stale engine.

## Key Fields/Parameters

- two independent `?v=` values:
  - the **asset version**: `widget/build.sh` hashes its own inputs (`widget/src/**`, both wasm binaries and their wasm-bindgen glue, the vendored `transformers.web.js` copy, the pinned transformers.js / onnxruntime-web / WebLLM versions, and `build.sh` itself), takes the first 12 hex characters and stamps `const EDDIE_ASSET_VERSION = "<hash>";` at the top of every bundle. Inputs only, never `dist/`, so the value is deterministic and independent of what it is stamped into
  - the **index version**: the `?v=` the CMS integration puts on `data-index-url` (the Hugo partial: the index's content hash under `assets/`, the build timestamp under `static/`)
- `lib/urls.js` exposes the stamp as `ASSET_VERSION` (null when the module is loaded outside a bundle, e.g. the Node tests, which simply means no `?v=`). It is the version used for `eddie-widget.js`, `eddie-worker.js`, `eddie-agent-worker.js`, `eddie-sw-*.js`, `eddie-lite.wasm`/`eddie-dense.wasm` and their glue, and `build.sh` writes the same `?v=` into the service workers' static import specifiers
- the index version is used for `index.ed`, its `index.<lane>.ed` sidecars and any site-bundled model files under a lane's `runtime.base_url`: what a single `eddie index` run writes together
- `lib/boot.js` resolves `eddie-widget.js` in this precedence: an explicit `?v=` on the boot script's own `src` (a site pinning a build), then `ASSET_VERSION`, then the index version (a bundle without the stamp)
- `build.sh` fails the build when any entry-point bundle is missing the stamp, since silently falling back to an unversioned URL would serve a stale engine out of the HTTP cache after an upgrade
- `hugo-module/example/_headers`: `/eddie/*` gets `max-age=31536000, immutable`; `eddie-boot.js`, the one URL a page names without a `?v=`, gets `max-age=300, must-revalidate`. `loader = "full"` sites also name `eddie-widget.js` directly and need the same rule for it (commented out in the example)

## Acceptance Criteria

- A returning visitor whose browser already has the runtime cached, arriving after a content-only redeploy (new index `?v=`, unchanged `dist/`), downloads the new index and nothing else: no widget, no wasm, no glue, no service worker script, and no service worker reinstall.
- A returning visitor arriving after an Eddie upgrade (new asset version, unchanged index) downloads the runtime and not the index.
- Every runtime asset URL the widget, the workers and the service workers build carries a `?v=`, so `/eddie/*` can be served `immutable` without pinning a stale build.
- Two builds from the same sources and binaries produce the same asset version; a change to any input changes it.

## Evidence

- `widget/build.sh` — asset version computation, `EDDIE_ASSET_VERSION` define, stamp check
- `widget/src/lib/urls.js`, `widget/test/urls.test.js`
- `widget/src/lib/boot.js`, `widget/test/boot.test.js`
- `widget/README.md` — [Script tag](../../../widget/README.md#script-tag)
- `hugo-module/example/_headers`, `hugo-module/layouts/partials/eddie/inject.html`
- `docs/plans/2026-09-01-efficient-defaults-pass2.md` — measured redeploy and upgrade scenarios

## Linked Tickets

- (none yet)
