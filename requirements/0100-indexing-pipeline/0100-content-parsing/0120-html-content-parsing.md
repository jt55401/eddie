# 0120 HTML Content Parsing

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can point the indexer at a Hugo `public/` output directory and it extracts text content from rendered HTML pages.

## Key Fields/Parameters

- `static-agent-cli index --content-dir <path> --format html --output <index.bin>`
- reads: `*.html` files recursively
- extracts: `<title>`, `<meta>` tags, main content area text
- strips: HTML tags, nav/footer boilerplate, scripts, styles

## Acceptance Criteria

- HTML is parsed and main content extracted (heuristic: largest text block or `<main>`/`<article>` element).
- Page title and meta description are preserved as metadata.
- Navigation, footer, and script content are excluded.
- Relative URLs in the source are preserved for linking.

## Evidence

- `tests/cli/test_html_parsing.rs`

## Linked Tickets

- (none yet)
