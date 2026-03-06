# CMS Gallery Refresh

This guide refreshes the README screenshot gallery by running every CMS integration in Docker and capturing an in-progress Eddie search with visible results.

## Prerequisites

- Docker running locally.
- Node/npm on host (used only for Playwright automation in `/tmp/eddie-gallery-tools`).
- Existing CMS E2E images (`eddie-<cms>-e2e-main`) OR run with `--build-images`.

## Run

From repo root:

```bash
bash scripts/capture-cms-gallery.sh
```

Build/update images first:

```bash
bash scripts/capture-cms-gallery.sh --build-images
```

## Output

Final README-sized images are written to:

- `assets/gallery/hugo-search-readme.png`
- `assets/gallery/astro-search-readme.png`
- `assets/gallery/docusaurus-search-readme.png`
- `assets/gallery/mkdocs-search-readme.png`
- `assets/gallery/eleventy-search-readme.png`
- `assets/gallery/jekyll-search-readme.png`

## Tuning

Override defaults with env vars:

```bash
EDDIE_GALLERY_QUERY="browser bug office hours" \
EDDIE_GALLERY_WAIT_FIRST_MS=70000 \
EDDIE_GALLERY_WAIT_MS=20000 \
EDDIE_GALLERY_WIDTH=640 \
EDDIE_GALLERY_HEIGHT=360 \
bash scripts/capture-cms-gallery.sh
```

Notes:

- `WAIT_FIRST_MS` is longer to absorb first-run model warmup.
- The script reuses a persistent Playwright profile in `/tmp` to keep browser/model caches warm.
- The capture flow uses `Ctrl+K` for most frameworks and click-open for MkDocs to avoid theme-native search hotkey conflicts.
- Installer source defaults to local repo scripts. To exercise published packages instead:
  - `EDDIE_INSTALL_SOURCE=registry EDDIE_PACKAGE_VERSION=0.2.2 bash scripts/capture-cms-gallery.sh`
- Run a subset during iteration with `EDDIE_GALLERY_CMS`, for example:
  - `EDDIE_GALLERY_CMS=jekyll bash scripts/capture-cms-gallery.sh`
  - `EDDIE_GALLERY_CMS=hugo,astro bash scripts/capture-cms-gallery.sh`
