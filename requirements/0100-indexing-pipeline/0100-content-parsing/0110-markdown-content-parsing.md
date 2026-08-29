# 0110 Markdown Content Parsing

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can point the indexer at a directory of markdown files and it extracts text content with frontmatter metadata (title, URL, date).

## Key Fields/Parameters

- `eddie index --content-dir <path> --cms <hugo|astro|docusaurus|eleventy|jekyll|mkdocs> --output <index.ed>`
- reads: `*.md` files recursively
- extracts: YAML/TOML frontmatter (`title`, `slug`, `url`, `date`, `description`)
- strips: markdown syntax, producing plain text segments
- one parser per `--cms` value (`src/parse/{hugo,astro,docusaurus,eleventy,jekyll,mkdocs}.rs`); each handles that generator's frontmatter and shortcode conventions

## Acceptance Criteria

- Frontmatter is parsed and preserved as metadata per chunk.
- Markdown syntax (headers, links, code blocks, images) is stripped to plain text.
- Files without frontmatter are indexed using filename as title.
- Non-UTF-8 files are skipped with a warning.
- `--cms` is required; there is no generic HTML-document parser (only source markdown is read, never rendered HTML output).

## Evidence

- `tests/cli/test_markdown_parsing.rs`

## Linked Tickets

- (none yet)
