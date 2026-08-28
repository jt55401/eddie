# 0410 Agent Ask Button and Answer Card

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor on a device with a capable WebGPU adapter, I can click an
"Ask" button (or press Shift+Enter) to get a streamed, cited answer from
the in-browser agent, displayed above the result list with source
citations, without leaving the search results behind.

## Key Fields/Parameters

- trigger: "Ask" button next to the search input, or Shift+Enter using the current query (no re-entry needed)
- gate: `data-agent-mode="auto"` and the device gate in [0300-qa-runtime's device gating story](../../0300-qa-runtime/0100-webgpu-detection/0110-webgpu-detection-fallback.md); `data-agent-mode="off"` hides the button unconditionally
- output: streamed answer text with `[n]` citations, plus a sources list of the cited pages with clickable links
- placement: answer card appears above the search results list; results remain visible below
- states: hidden (default), consent (see [0320](../0300-download-progress/0320-download-consent.md)), downloading model (progress bar), generating (streaming, stop button), complete (answer + sources), error (graceful message with retry)

## Acceptance Criteria

- The Ask affordance is rendered only when the device gate passes and `data-agent-mode` is not `off` (see 0300-qa-runtime/0110); otherwise it is absent and search is unaffected.
- Clicking Ask (or Shift+Enter) uses the current search query.
- The generated answer includes a sources list with clickable page links, and inline `[n]` citations that resolve to that list.
- The answer streams token-by-token as the model generates it, with a visible stop button that aborts generation.
- A new query or a new Ask while generation is in progress aborts the previous generation before starting the next.
- Answer card states transition exactly as listed above; an error state never leaves the UI stuck on a spinner.

## Evidence

- `tests/integration/test_qa_mode.js`

## Linked Tickets

- (none yet)
