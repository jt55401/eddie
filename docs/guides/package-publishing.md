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
    { "path": "integrations/hugo/npm" },
    { "path": "integrations/astro/npm" },
    { "path": "integrations/docusaurus/npm" },
    { "path": "integrations/eleventy/npm" }
  ],
  "pypi": [
    { "path": "integrations/cli/pypi" },
    { "path": "integrations/mkdocs/pypi" }
  ],
  "rubygems": [
    { "path": "integrations/cli/gem" },
    { "path": "integrations/jekyll/gem" }
  ]
}
```

Each target path should contain exactly one package:

- npm: `package.json`
- PyPI: `pyproject.toml` (or `setup.py`)
- RubyGems: exactly one `*.gemspec`

For npm targets, `build` is optional and runs before validation/publish. Use it for generated packages (for example `wasm-pack` output under `widget/pkg`).

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

- `eddie-cli`
- `eddie-jekyll`

## 4) Release flow

### Tag-based publish

Push a tag (example: `v0.3.0`).

All three publish workflows trigger on `v*` tags and publish targets from `.github/publish-packages.json`.

After publish, `post-publish-registry-smoke.yml` runs CMS Docker E2E against registry packages + release runtime assets to verify install, indexing, and CLI search all succeed.

### Manual publish / dry-run

Run each workflow with `workflow_dispatch`.

Optional input:

- `package_path`: publish one package path directly
- `dry_run`:
  - npm: runs `npm publish --dry-run`
  - PyPI: builds and runs `twine check`, skips upload
  - RubyGems: builds gem, skips `gem push`

For registry smoke tests:

- run `post-publish-registry-smoke.yml` with input `version` (for example `0.2.0`)
- or rely on automatic trigger from tag pushes (`v*`)

## Secrets

- Required for OIDC publishing: none
- Optional: if private npm dependency installs are needed in CI, add a read-only `NPM_TOKEN`
