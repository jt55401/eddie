# Package Publishing Guide (npm, PyPI, RubyGems)

This repo now includes OIDC-first release workflows:

- `.github/workflows/publish-npm.yml`
- `.github/workflows/publish-pypi.yml`
- `.github/workflows/publish-rubygems.yml`
- `.github/workflows/post-publish-registry-smoke.yml`

All three workflows read publish targets from `.github/publish-packages.json`.

## 1) Configure publish targets

Edit `.github/publish-packages.json` and add package directories:

```json
{
  "npm": [
    {
      "path": "widget/pkg",
      "build": "bash widget/build.sh"
    },
    { "path": "integrations/cli/npm" },
    { "path": "integrations/hugo/npm", "assets_dir": "integrations/hugo/npm/assets" },
    { "path": "integrations/astro/npm", "assets_dir": "integrations/astro/npm/assets" },
    { "path": "integrations/docusaurus/npm", "assets_dir": "integrations/docusaurus/npm/assets" },
    { "path": "integrations/eleventy/npm", "assets_dir": "integrations/eleventy/npm/assets" }
  ],
  "pypi": [
    { "path": "integrations/cli/pypi" },
    { "path": "integrations/mkdocs/pypi", "assets_dir": "integrations/mkdocs/pypi/src/eddie_mkdocs/assets" }
  ],
  "rubygems": [
    { "path": "integrations/cli/gem" },
    { "path": "integrations/jekyll/gem", "assets_dir": "integrations/jekyll/gem/assets" }
  ]
}
```

Each target path should contain exactly one package:

- npm: `package.json`
- PyPI: `pyproject.toml` (or `setup.py`)
- RubyGems: exactly one `*.gemspec`

For npm targets, `build` is optional and runs before validation/publish. Use it for generated packages (for example `wasm-pack` output under `widget/pkg`).

### Widget runtime assets (`assets_dir`)

The CMS installer packages (`integrations/{hugo,astro,docusaurus,eleventy}/npm`,
`integrations/jekyll/gem`, `integrations/mkdocs/pypi`) each ship a copy of the
built widget runtime -- every file named in `widget/assets.list` (the boot
loader, the full widget, the page-worker fallbacks, the lite/dense WASM
modules and their glue, and the tiered service workers), plus a copy of
`assets.list` itself so each package's install script can read the list
back instead of hardcoding it -- so their install scripts can drop it
straight into a site. These files are **generated, never committed**.
`ci.yml`'s `packaging-check` job fails the build if one is checked in
(matched by directory, so it doesn't need updating when the asset list
does), and `.gitignore` excludes the whole `assets/` directory under each
package. This was a real problem before: five of six npm packages plus
the mkdocs/jekyll packages shipped pre-committed binaries with no build step
and no drift check, so a widget fix could ship stale bits to every package
except `widget/pkg`.

Every publish workflow (`publish-npm.yml`, `publish-pypi.yml`,
`publish-rubygems.yml`) now has a `build-widget-dist` job that runs
`widget/build.sh` exactly once and uploads `dist/` as a workflow artifact.
Each matrix target with an `assets_dir` entry downloads that artifact and
copies the files named in `widget/assets.list` (plus the manifest itself)
in before validating/building/publishing, so every package in one publish
run ships identical bits.

For local testing, `scripts/sync-integration-assets.sh` does the same thing
outside CI: it builds the widget once and copies `dist/` (and the manifest)
into every `assets_dir` from `.github/publish-packages.json`. Run it (or
`--no-build` if `dist/` is already current) before manually testing an
installer script.

## 2) Create a GitHub Environment

Create an environment named `release` in this repo:

1. Repo Settings -> Environments -> New environment
2. Name: `release`
3. Add protection rules as needed (recommended: required reviewers + tag-based release policy)

The workflows publish from this environment.

## 3) Configure trusted publishers in each registry

No long-lived publish token is required for normal publishing.

### npm

For each npm package, add a trusted publisher that matches:

- GitHub repository: this repo
- Workflow file: `.github/workflows/publish-npm.yml`
- Environment: `release`

At minimum this now includes:

- `@jt55401/eddie-cli`
- `@jt55401/eddie-hugo`
- `@jt55401/eddie-astro`
- `@jt55401/eddie-docusaurus`
- `@jt55401/eddie-eleventy`

### PyPI

For each PyPI project, add a trusted publisher that matches:

- GitHub repository: this repo
- Workflow file: `.github/workflows/publish-pypi.yml`
- Environment: `release` (recommended)

At minimum this now includes:

- `jt55401-eddie-cli`
- `eddie-mkdocs`

### RubyGems

For each gem, add a trusted publisher that matches:

- GitHub repository: this repo
- Workflow file: `.github/workflows/publish-rubygems.yml`
- Environment: `release`

At minimum this now includes:

- `jt55401-eddie-cli`
- `eddie-jekyll`

## 4) Release flow

### Tag-based publish

Push a tag (example: `v0.4.0`).

All three publish workflows trigger on `v*` tags and publish targets from `.github/publish-packages.json`.

`post-publish-registry-smoke.yml` triggers on `workflow_run` completion of
the three publish workflows (not on the tag push directly), so it doesn't
race a manual-approval gate on the `release` environment. It fires once per
publish workflow that completes and polls every registry/asset a given CMS
needs for up to ~60 minutes per check (200-minute job timeout) before
running CMS Docker E2E against the published packages. If your approval
takes longer than that, re-run it manually (see below) once publishing
actually finishes.

### Manual publish / dry-run

Run each workflow with `workflow_dispatch`.

Optional input:

- `package_path`: publish one package path directly
- `dry_run`:
  - npm: runs `npm publish --dry-run`
  - PyPI: builds and runs `twine check`, skips upload
  - RubyGems: builds gem, skips `gem push`

For registry smoke tests:

- it normally runs on its own, triggered by each publish workflow finishing
- to re-run it by hand (for example after a slow manual approval), use
  `workflow_dispatch` with input `version` (for example `0.4.0`)

## Secrets

- Required for OIDC publishing: none
- Optional: if private npm dependency installs are needed in CI, add a read-only `NPM_TOKEN`
