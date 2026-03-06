# Package Publishing Guide (npm, PyPI, RubyGems)

This repo now includes OIDC-first release workflows:

- `.github/workflows/publish-npm.yml`
- `.github/workflows/publish-pypi.yml`
- `.github/workflows/publish-rubygems.yml`

All three workflows read publish targets from `.github/publish-packages.json`.

## 1) Configure publish targets

Edit `.github/publish-packages.json` and add package directories:

```json
{
  "npm": [
    { "path": "integrations/astro/npm" }
  ],
  "pypi": [
    { "path": "integrations/mkdocs/python" }
  ],
  "rubygems": [
    { "path": "integrations/jekyll/ruby" }
  ]
}
```

Each target path should contain exactly one package:

- npm: `package.json`
- PyPI: `pyproject.toml` (or `setup.py`)
- RubyGems: exactly one `*.gemspec`

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

### PyPI

For each PyPI project, add a trusted publisher that matches:

- GitHub repository: this repo
- Workflow file: `.github/workflows/publish-pypi.yml`
- Environment: `release` (recommended)

### RubyGems

For each gem, add a trusted publisher that matches:

- GitHub repository: this repo
- Workflow file: `.github/workflows/publish-rubygems.yml`
- Environment: `release`

## 4) Release flow

### Tag-based publish

Push a tag (example: `v0.3.0`).

All three publish workflows trigger on `v*` tags and publish targets from `.github/publish-packages.json`.

### Manual publish / dry-run

Run each workflow with `workflow_dispatch`.

Optional input:

- `package_path`: publish one package path directly
- `dry_run`:
  - npm: runs `npm publish --dry-run`
  - PyPI: builds and runs `twine check`, skips upload
  - RubyGems: builds gem, skips `gem push`

## Secrets

- Required for OIDC publishing: none
- Optional: if private npm dependency installs are needed in CI, add a read-only `NPM_TOKEN`
