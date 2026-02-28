# 0220 Result Ranking and Snippets

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see search results with a page title, a text snippet from the matching chunk, and a clickable link to the page.

## Key Fields/Parameters

- per result: `{ title, url, section, snippet, score }`
- snippet: the matching chunk's text, truncated to ~150 chars with ellipsis
- link: the page URL from the index metadata

## Acceptance Criteria

- Each result includes the page title, URL, section heading, and a text snippet.
- Results link directly to the source page on the site.
- Duplicate pages are deduplicated (best-scoring chunk per page shown).

## Evidence

- `tests/wasm/test_result_format.rs`

## Linked Tickets

- (none yet)
