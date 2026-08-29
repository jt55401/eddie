# 0210 GitHub Actions Workflow

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner using GitHub Pages (or any static host), I can add a
GitHub Actions step that indexes my content and deploys the index
alongside my site, pinned to a specific Eddie version so the same commit
always produces the same index.

## Key Fields/Parameters

- template: `.github/workflows/example-hugo.yml` in this repo, and the equivalent snippet in README.md's GitHub Actions section
- CLI install: `npx @jt55401/eddie-cli@<pinned-version>` (never `@latest`); the launcher downloads the matching platform binary and verifies it against that release's `SHA256SUMS` before running it
- inputs: `--content-dir`, `--cms`, `--output`, plus any indexing flags (see the CLI reference in README.md)
- integration point: runs after content is ready, before the site build (or after, if the index only needs to land in the build output directory)

## Acceptance Criteria

- The documented template pins both the site generator's own version (for example Hugo) and the Eddie CLI version explicitly; neither uses `latest`.
- The Eddie CLI launcher verifies the downloaded binary's checksum against `SHA256SUMS` before marking it executable, and refuses to run on a mismatch.
- The action produces the index file and copies it to the site's output/static directory.
- The same commit, re-run on a later date, produces a byte-identical index (given unchanged content), because nothing in the pipeline resolves an unpinned "latest".

## Evidence

- `.github/workflows/example-hugo.yml`
- `docs/guides/github-actions.md`

## Linked Tickets

- (none yet)
