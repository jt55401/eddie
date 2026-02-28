# 0110 Hugo Integration

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a Hugo site owner, I can add static-agent to my build process and embed the widget in my theme with minimal configuration.

## Key Fields/Parameters

- indexer input: Hugo `content/` directory (markdown) or `public/` (rendered HTML)
- indexer output: `static/static-agent-index.bin` (served as a static asset)
- widget embed: `<script src="/static-agent-widget.js"></script>` in the theme's `baseof.html`
- config: `static-agent.toml` in the site root

## Acceptance Criteria

- A Hugo site owner can add 2-3 lines to their theme to enable the widget.
- The index file is placed in Hugo's `static/` directory and served as-is.
- The widget JS bundle is self-contained (no external CSS/JS dependencies beyond the WASM).
- Documentation includes a step-by-step Hugo integration guide.

## Evidence

- `docs/guides/hugo.md`
- `examples/hugo/`

## Linked Tickets

- (none yet)
