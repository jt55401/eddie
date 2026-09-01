<!-- SPDX-License-Identifier: GPL-3.0-only -->
# User guide

How to add Eddie to a static site, configure it, and deploy it.

For every command and flag, see the [reference](reference.md). To measure
and improve result quality, see [tuning](tuning.md).

## Contents

- [Install the CLI](#install-the-cli)
- [Index your content](#index-your-content)
- [Add the widget](#add-the-widget)
- [Configure the widget](#configure-the-widget)
- [What visitors can change](#what-visitors-can-change)
- [Deploy](#deploy)
- [Build it in CI](#build-it-in-ci)
- [Per-CMS guides](#per-cms-guides)

## Install the CLI

Download a binary from the [releases page](https://github.com/jt55401/eddie/releases),
or install through the package manager you already use:

```bash
npm install --save-dev @jt55401/eddie-cli     # Node
pip install eddie-cli                          # Python
gem install jt55401-eddie-cli                  # Ruby
```

Each installer downloads the binary for your platform and checks it against
the release checksums.

## Index your content

Point Eddie at your content directory and say which site generator wrote it:

```bash
eddie index --content-dir content/ --cms hugo --output static/eddie/index.ed --preset balanced
```

This writes `index.ed`. Deploy it like any other file.

**Which `--cms` to use.** Eddie reads your source files, so it needs to know
the format: `hugo`, `astro`, `docusaurus`, `eleventy`, `jekyll` or `mkdocs`.

If most of your text lives in templates rather than in content files, index
the built HTML instead:

```bash
eddie index --content-dir public/ --cms html --output public/eddie/index.ed --preset balanced
```

Point `--content-dir` at the output directory, not the source. Eddie then
skips navigation, headers and footers, and ignores pages marked `noindex`.

**Which `--preset` to use.** A preset chooses the models so you do not have
to:

| Preset | Good for | Visitor download |
|---|---|---|
| `fast` | Small sites, lowest cost to visitors | about 45 MB |
| `balanced` | Most sites. Start here | about 45 MB |
| `quality` | Large sites, best results on browsers with WebGPU | 45 MB, or about 900 MB on WebGPU |
| `gpu` | Same as `quality`, but indexes faster on a CUDA machine | same as `quality` |

Rebuild the index whenever your content changes. It is part of your site
build, like generating HTML.

## Add the widget

Copy the widget files into your site, then add one script tag:

```html
<script src="/eddie/eddie-boot.js" data-index-url="/eddie/index.ed"></script>
```

`eddie-boot.js` is 3 KB. It draws the search button and the Ctrl/Cmd+K
shortcut, and it fetches the rest only when someone opens the search box.

If you would rather load everything on every page view, point the tag at
`eddie-widget.js` instead. Both read the same `data-*` attributes.

The CMS installers copy the widget files for you. So does the Hugo module.

## Configure the widget

There is no configuration file. The widget reads `data-*` attributes on its
own script tag:

```html
<script src="/eddie/eddie-boot.js"
        data-index-url="/eddie/index.ed"
        data-position="bottom-right"
        data-theme="auto"
        data-top-k="8"
        data-agent-mode="auto"
></script>
```

| Attribute | Values | What it does |
|---|---|---|
| `data-index-url` | a URL | Where the index file is. Defaults to `index.ed` next to the script |
| `data-position` | `top-left`, `top-right`, `bottom-left`, `bottom-right` | Where the search button sits |
| `data-theme` | `light`, `dark`, `auto` | `auto` follows the visitor's system setting |
| `data-top-k` | a number | How many results to show. Default 8 |
| `data-qa-mode` | `off`, `auto`, `always` | Whether to show an answer card above the results |
| `data-agent-mode` | `off`, `auto` | Whether to offer the in-browser answer model |
| `data-agent-model` | `auto`, `light`, `quality`, or a model id | Which answer model to use |
| `data-dense-runtime` | `auto`, `wasm`, `webgpu`, `off` | Which search model to use, or `off` for keywords only |
| `data-persist` | `auto`, `off` | Whether to keep the engine loaded as visitors move between pages |
| `data-warm` | `auto`, `off`, `always` | When to load the engine, relative to the visitor opening search |
| `data-consent-text` | text | Replaces the wording of the download prompt |

On Hugo, set these as `[params.eddie]` in `hugo.toml` and the module writes
the attributes for you. See the [Hugo guide](guides/hugo.md).

The full list, including the attributes most sites never need, is in the
[reference](reference.md#widget-attributes).

## What visitors can change

The search box has a gear icon. Behind it, each visitor can choose:

- **Search model** — which model to download, or none at all
- **Answers** — off, a light model, or a larger one
- **Preload** — whether to load the engine before they search
- **Between pages** — whether to keep it loaded as they browse
- **Downloads** — how much is stored on their device, with a button to delete it

Their choices are remembered in their browser.

Your settings are the ceiling. A visitor can always choose less than you
offer, and can pick among the options you left open, but cannot switch on
something you turned off. If you set `data-agent-mode="off"`, no visitor
sees an answer model.

## Deploy

Deploy the index and the widget files as static files. Nothing else is
needed.

**Caching.** Every file Eddie fetches carries a `?v=` in its URL, so you can
cache them for a year and still ship updates. Copy
[`hugo-module/example/_headers`](../hugo-module/example/_headers) into your
site if you host on Cloudflare Pages or Netlify. It sets a one-year cache on
everything except the small loader script, which has to keep checking for
updates.

The runtime assets and your index carry *different* versions. Publishing new
content changes the index URL only, so returning visitors download the new
index and keep the engine they already have.

**Compression.** The release pipeline ships a `.br` and a `.gz` next to each
runtime file listed in [`widget/assets.list`](../widget/assets.list), which
is the authoritative list of what a deployment needs. Reference the plain
filenames in your HTML and let your host serve the compressed bytes through
normal content negotiation. Do not link to the `.br` files directly unless
your host also sets `Content-Encoding`.

## Build it in CI

Use [`.github/workflows/example-hugo.yml`](../.github/workflows/example-hugo.yml)
as a starting point. Pin the CLI version rather than tracking the latest
release:

```yaml
- name: Build the Eddie index
  run: npx -y @jt55401/eddie-cli@0.4.3 index --cms hugo --content-dir content/ --output public/eddie/index.ed
```

The launcher checks the downloaded binary against the release checksums
before running it. The [GitHub Actions guide](guides/github-actions.md) has
the full pipeline.

## Per-CMS guides

- [Hugo](guides/hugo.md) — the module, its parameters, and the init script
- [GitHub Actions](guides/github-actions.md) — building the index in CI
- [Screenshot gallery](guides/cms-gallery.md) — Eddie running on each CMS

## Related documents

- [Reference](reference.md) — every command, flag, model and format detail
- [Tuning](tuning.md) — measuring and improving result quality
- [Benchmarks](benchmarks.md) — how Eddie is measured
