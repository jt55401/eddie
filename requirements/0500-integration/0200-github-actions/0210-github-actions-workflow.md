# 0210 GitHub Actions Workflow

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner using GitHub Pages, I can add a GitHub Action that indexes my content and deploys the index alongside my site.

## Key Fields/Parameters

- action: `eddie/index-action@v1` (or inline step using the CLI binary)
- inputs: `content-dir`, `output-path`, `model` (optional)
- artifacts: `eddie-index.bin`, `eddie-widget.js`, `eddie.wasm`
- integration point: runs after site build, before deploy

## Acceptance Criteria

- A reusable GitHub Action (or documented workflow snippet) is provided.
- The action downloads the CLI binary (pre-built release) or builds from source.
- The action produces the index file and copies it to the site output directory.
- The workflow works with Hugo, Jekyll, and plain HTML sites (generic content-dir input).
- Build time for ~100 pages is under 2 minutes.

## Evidence

- `.github/workflows/example-hugo.yml`
- `docs/guides/github-actions.md`

## Linked Tickets

- (none yet)
