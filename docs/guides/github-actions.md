# GitHub Actions Guide

This repository ships three workflows:

- `ci.yml`: runs Rust tests and verifies widget build output on pushes and pull requests.
- `release.yml`: builds release artifacts for Eddie and publishes them on `v*` tags.
- `publish-hugo-module.yml`: optionally syncs `hugo-module/` to `jt55401/eddie-hugo`.

## Release artifacts

When you push a tag like `v0.1.0`, `release.yml` produces and uploads:

- `eddie-linux-amd64`
- `eddie.wasm`
- `eddie-wasm.js`
- `eddie-worker.js`
- `eddie-widget.js`
- `eddie-hugo-module.tar.gz`
- `SHA256SUMS`

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
