# 0310 Model Download Progress

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see a progress indicator when a model is downloading
for the first time, so I understand why search or the agent isn't instant.

## Key Fields/Parameters

- triggers: first search (the index's dense lane model, size depends on the preset the site was indexed with — see the model table in README.md), first Ask (the agent model, see [0300-qa-runtime's agent story](../../0300-qa-runtime/0200-llm-synthesis/0210-llm-answer-synthesis.md) for size by `data-agent-model`)
- display: `status` events (`loading_index`, `loading_model {file, progress}`, `ready {lanes, arms}`) drive a progress bar with percentage and size; falls back to an indeterminate spinner when `Content-Length` is missing
- caching: model files are cached in IndexedDB keyed by `repo@revision/file`; after first download, subsequent loads read from cache

## Acceptance Criteria

- Progress bar shows bytes downloaded vs total for each model file, when the server reports `Content-Length`.
- Search model download is triggered on first search, not on page load.
- Agent model download is triggered on first Ask use (after consent, see [0320](0320-download-consent.md)), not on first search.
- On subsequent visits, cached models load without showing download progress.
- If a model fetch fails after its retry, an error message is shown with a retry action; a failed dense lane is reported once and search continues with the remaining arms.

## Evidence

- `tests/integration/test_download_progress.js`

## Linked Tickets

- (none yet)
