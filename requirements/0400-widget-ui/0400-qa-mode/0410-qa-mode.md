# 0410 Ask Button and Answer Card

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I can click an "Ask" button (or press Shift+Enter) to get a short AI-generated answer synthesized from my search results, displayed above the result list with source citations.

## Key Fields/Parameters

- trigger: "Ask" button to the right of the search input, or Shift+Enter keyboard shortcut
- requires: WebGPU support (`navigator.gpu`)
- output: 1-3 sentence answer + "Sources:" section with clickable links
- placement: answer card appears above the search results list; results remain visible below
- search results already available (search is instant, runs on typing)

## Acceptance Criteria

- Ask button is only rendered when WebGPU is detected (no button = no Q&A, search unaffected).
- Clicking Ask (or Shift+Enter) uses the current search query — no re-entry needed.
- The generated answer includes a "Sources:" section with clickable page links.
- The answer is streamed token-by-token as the LLM generates it (pulsing cursor during generation).
- Ask button shows a spinner while the model is downloading or generating.
- Answer card states: hidden (default), downloading model (progress bar), generating (streaming), complete (answer + sources), error (graceful message).
- A new Ask on a different query replaces the previous answer card.

## Evidence

- `tests/integration/test_qa_mode.js`

## Linked Tickets

- (none yet)
