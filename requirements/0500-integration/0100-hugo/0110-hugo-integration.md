# 0110 Hugo Integration

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a Hugo site owner, I can add eddie to my build process and embed the
widget in my theme with minimal configuration, and without hand-writing a
`<script>` tag's `data-*` attributes.

## Key Fields/Parameters

- indexer input: Hugo `content/` directory (markdown, via `--cms hugo`)
- indexer output: `static/eddie/index.ed` (served as a static asset), format v5
- widget embed, turnkey path: the `eddie-hugo` Hugo Module's `layouts/partials/eddie/inject.html` partial, called once from the theme; it renders the `<script>` tag and every `data-*` attribute from `[params.eddie]` in `hugo.toml`
- widget embed, default path: the module partial (or a hand-written tag) emits `<script src="/eddie/eddie-boot.js"></script>`; this ~3 KB boot loader draws the trigger button and Ctrl/Cmd+K shortcut itself and forwards every `data-*` attribute unchanged when it fetches `eddie-widget.js` on first interaction (or, for a returning visitor who has searched or consented to a model before on this browser, after `load`)
- widget embed, always-on path: `loader = "full"` in `[params.eddie]` (or a hand-written `<script src="/eddie-widget.js">`) puts the full widget on every page view instead of the boot loader — same `data-*` attributes either way (see README.md's Configuration section)
- config: no `eddie.toml` file exists; site-owner configuration is CLI flags at index time and `[params.eddie]` (or raw `data-*` attributes) at widget-embed time
- the module partial builds the index URL through `relURL` (so it resolves correctly under a baseURL path prefix) and appends a cache-busting `?v=` derived from the index file's content hash (when it lives under `assets/`) or the build timestamp (the common case, when it lives under `static/`)

## Acceptance Criteria

- A Hugo site owner using the module can add one `go.mod` import and one partial call to enable the widget.
- The index file is placed in Hugo's `static/` directory and served as-is.
- The widget JS bundle is self-contained (no external CSS/JS dependencies beyond the WASM, and beyond transformers.js/WebLLM when those lanes are used).
- The rendered `data-index-url` resolves correctly for a site served under a subpath (`https://example.com/blog/`), not just at the domain root.
- Documentation includes a step-by-step Hugo integration guide.
- With the default `loader = "boot"`, a plain page view fetches only the boot script, not `eddie-widget.js` or any WASM/model asset — the module partial's default keeps the page-view cost at the boot script's size, not the full widget's.

## Evidence

- `docs/guides/hugo.md`
- `hugo-module/`

## Linked Tickets

- (none yet)
