# 0410 Q&A Mode

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I can switch to Q&A mode to get a short generated answer to my question, with citations to the source pages.

## Key Fields/Parameters

- toggle: tab or button in the modal ("Search" / "Q&A beta")
- requires: WebGPU support
- output: 1-3 sentence answer + cited source page links
- state: Q&A mode remembers the current query from search mode

## Acceptance Criteria

- Q&A tab is only shown when WebGPU is detected.
- Switching to Q&A mode reuses the current search query.
- The generated answer includes explicit source links.
- A "beta" label or indicator communicates this feature is experimental.
- The answer is streamed token-by-token as the LLM generates it.

## Evidence

- `tests/integration/test_qa_mode.js`

## Linked Tickets

- (none yet)
