# 0310 Model Download Progress

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see a progress indicator when a model is downloading for the first time, so I understand why search or Q&A isn't instant.

## Key Fields/Parameters

- triggers: first search (embedding model ~23MB), first Q&A (LLM ~1-2GB)
- display: progress bar with percentage and size (e.g., "Loading search model... 12/23 MB")
- caching: after first download, model loads from browser cache instantly

## Acceptance Criteria

- Progress bar shows bytes downloaded vs total for each model.
- Search model download is triggered on first search, not on page load.
- LLM model download is triggered on first Q&A use, not on first search.
- On subsequent visits, cached models load without showing download progress.
- If download fails, an error message is shown with a retry option.

## Evidence

- `tests/integration/test_download_progress.js`

## Linked Tickets

- (none yet)
