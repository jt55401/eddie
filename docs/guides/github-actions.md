# GitHub Actions Guide

This repository ships three workflows:

- `ci.yml`: runs Rust tests and verifies widget build output on pushes and pull requests.
- `release.yml`: builds release artifacts for Eddie and publishes them on `v*` tags.
- `publish-hugo-module.yml`: optionally syncs `hugo-module/` to `jt55401/eddie-hugo`.

## Release artifacts

When you push a tag like `v0.2.1`, `release.yml` produces and uploads:

- `eddie-linux-amd64`
- `eddie.wasm`
- `eddie.wasm.br`
- `eddie.wasm.gz`
- `eddie-wasm.js`
- `eddie-wasm.js.br`
- `eddie-wasm.js.gz`
- `eddie-worker.js`
- `eddie-worker.js.br`
- `eddie-worker.js.gz`
- `eddie-widget.js`
- `eddie-widget.js.br`
- `eddie-widget.js.gz`
- `ASSET_SIZES.md`
- `asset-sizes.csv`
- `eddie-hugo-module.tar.gz`
- `SHA256SUMS`

The build installs `binaryen` and runs `wasm-opt -Oz` on the generated WASM
artifact before packaging release assets.

## Size budgets

`ci.yml` runs `scripts/report-asset-sizes.sh`, which reports raw/gzip/brotli
sizes and enforces budgets for `eddie.wasm`:

- `WASM_RAW_BUDGET_BYTES` (default `3400000`)
- `WASM_GZIP_BUDGET_BYTES` (default `1100000`)
- `WASM_BROTLI_BUDGET_BYTES` (default `800000`)

## Hugo module publishing

`publish-hugo-module.yml` is designed to publish the module into the separate
`jt55401/eddie-hugo` repository.

1. Create a classic PAT (or fine-grained token) with write access to `jt55401/eddie-hugo`.
2. Add it to this repo as `EDDIE_HUGO_PAT`.
3. Push a release tag (`v*`) or run the workflow manually.

If `EDDIE_HUGO_PAT` is missing, the workflow prints a skip message and exits
without failing the run.

## Example for site owners

Use `.github/workflows/example-hugo.yml` in your Hugo site repo as a starting
point for indexing content and building the site in CI.
