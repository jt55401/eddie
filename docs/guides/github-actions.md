# GitHub Actions Guide

This repository ships eight workflows:

- `ci.yml`: `cargo fmt --check`, `cargo clippy -D warnings`, native tests, a `wasm32-unknown-unknown` build, widget build + size budgets, and a packaging regression check, on pushes and pull requests.
- `release.yml`: builds the CLI for five platforms and publishes them on `v*` tags.
- `publish-hugo-module.yml`: syncs `hugo-module/` to `jt55401/eddie-hugo`, skipped when `EDDIE_HUGO_DEPLOY_KEY` is not configured.
- `publish-npm.yml`: publishes npm packages with trusted publishing (OIDC).
- `publish-pypi.yml`: publishes PyPI packages with trusted publishing (OIDC).
- `publish-rubygems.yml`: publishes gems with trusted publishing (OIDC).
- `post-publish-registry-smoke.yml`: runs CMS Docker E2E in `registry` mode after the three publish workflows finish.
- `example-hugo.yml`: a template for a site repo's own CI (see "Example for site owners" below).

See also: [Package Publishing Guide](package-publishing.md) for setup details.

## Toolchain pin

`rust-toolchain.toml` at the repo root pins Rust `1.93.1` plus the
`wasm32-unknown-unknown` target. Every workflow installs this exact version
(no floating `stable` channel) so a Rust point release can't silently change
what a tag produces. Update it deliberately when you bump the toolchain, and
keep `~/.cargo/bin/{cargo,rustc,...}` on the same version locally (see
CLAUDE.md's Rust toolchain notes) so `cargo build`/`cargo clippy` agree.

## Release artifacts

When you push a tag like `v0.4.0`, `release.yml` builds the CLI natively on
five runners (no cross-compilation) and produces:

- `eddie-linux-x86_64` (`ubuntu-22.04`)
- `eddie-linux-aarch64` (`ubuntu-24.04-arm`)
- `eddie-macos-aarch64` (`macos-14`)
- `eddie-macos-x86_64` (`macos-15-intel`)
- `eddie-windows-x86_64.exe` (`windows-2022`)
- `eddie.wasm`, `eddie.wasm.br`, `eddie.wasm.gz`
- `eddie-wasm.js`, `eddie-wasm.js.br`, `eddie-wasm.js.gz`
- `eddie-worker.js`, `eddie-worker.js.br`, `eddie-worker.js.gz`
- `eddie-widget.js`, `eddie-widget.js.br`, `eddie-widget.js.gz`
- `ASSET_SIZES.md`, `asset-sizes.csv`
- `eddie-hugo-module.tar.gz`
- `SHA256SUMS`

The `build` job runs the five platform builds in parallel and uploads each
binary as a workflow artifact; the `assemble` job (Ubuntu only) downloads
them, builds the widget, runs `wasm-opt -Oz` on the WASM artifact, computes
`SHA256SUMS` over everything, and publishes the GitHub Release. A
`workflow_dispatch` run does the same build and assembly as a dry run but
skips the publish step (gated on `startsWith(github.ref, 'refs/tags/')`).

The npm, gem, and PyPI CLI launchers (`integrations/cli/*`) verify the
downloaded binary against `SHA256SUMS` before marking it executable and
refuse to run a binary that doesn't match.

CUDA builds are not part of this matrix. Build one locally with
`cargo build --release --features cuda` when you have a CUDA toolchain
available.

## Size budgets

`ci.yml` runs `scripts/report-asset-sizes.sh`, which reports raw/gzip/brotli
sizes and enforces budgets for `eddie.wasm`:

- `WASM_RAW_BUDGET_BYTES` (default `3400000`)
- `WASM_GZIP_BUDGET_BYTES` (default `1100000`)
- `WASM_BROTLI_BUDGET_BYTES` (default `800000`)

## Hugo module publishing

`publish-hugo-module.yml` publishes the module into the separate
`jt55401/eddie-hugo` repository over SSH.

1. Generate an SSH deploy key and add the public half as a deploy key (with
   write access) on `jt55401/eddie-hugo`.
2. Add the private half to this repo as the secret `EDDIE_HUGO_DEPLOY_KEY`.
3. Push a release tag (`v*`) or run the workflow manually.

A `check-secret` job checks whether `EDDIE_HUGO_DEPLOY_KEY` is set before
anything else runs. If it isn't, the workflow logs a skip message and exits
without failing the run. No PAT is involved, and no separate `EDDIE_HUGO_PAT`
secret is used.

## Registry smoke tests

`post-publish-registry-smoke.yml` triggers on `workflow_run` completion of
`publish-npm.yml`, `publish-pypi.yml`, and `publish-rubygems.yml` (plus a
manual `workflow_dispatch` with a `version` input), rather than racing the
same tag push those three workflows respond to. Because `workflow_run` fires
once per source workflow, the smoke suite runs up to three times per
release; each run polls every registry/asset it needs (up to ~60 minutes per
check, job timeout 200 minutes), so whichever run happens to land last is the
one that actually succeeds. This is deliberately generous enough to survive
a "required reviewers" approval gate on the release environment. If your
approval takes longer than that, re-run `post-publish-registry-smoke.yml`
manually with `workflow_dispatch` once publishing finishes.

## Example for site owners

Use `.github/workflows/example-hugo.yml` in your Hugo site repo as a starting
point for indexing content and building the site in CI. It pins both the
Hugo version and the `@jt55401/eddie-cli` version explicitly (no `latest`) so
a CI re-run of the same commit can't silently change chunking behavior or
index format. Bump both deliberately, together, when you upgrade.
