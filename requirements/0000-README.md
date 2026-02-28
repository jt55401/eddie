# 0000 Requirements Architecture

[Requirements Register](../requirements.md)
[Requirements Changelog](CHANGELOG.md)

## Numbering Scheme

- Use 4-digit prefixes for all folders/files.
- Leave gaps (`0110`, `0120`, `0130`) so new items can be inserted.
- Each area has:
  - `0000-high-level-requirements.md`
  - subfolders per functional area
  - one markdown file per story

## Area Map

- [0100-indexing-pipeline](0100-indexing-pipeline/0000-high-level-requirements.md) — CLI content parsing, chunking, embedding
- [0200-search-runtime](0200-search-runtime/0000-high-level-requirements.md) — WASM query embedding and vector search
- [0300-qa-runtime](0300-qa-runtime/0000-high-level-requirements.md) — Optional WebGPU LLM answer synthesis
- [0400-widget-ui](0400-widget-ui/0000-high-level-requirements.md) — JS widget, modal, states, download progress
- [0500-integration](0500-integration/0000-high-level-requirements.md) — Static site generators, CI/CD, GitHub Actions
- [0600-configuration](0600-configuration/0000-high-level-requirements.md) — Model selection, theming, site-level config

## Story Rules

1. One story file describes one behavior.
2. Include key fields/parameters, acceptance criteria, evidence, and linked tickets.
3. Keep links relative and valid.
