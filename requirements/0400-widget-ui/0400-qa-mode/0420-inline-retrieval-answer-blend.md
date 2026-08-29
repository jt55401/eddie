# 0420 Inline Retrieval Answer Blend

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I can see a short answer blended into my search results
for a factual-shaped query, drawn straight from retrieved content, without
waiting for any model download and without pressing a separate "Ask"
button.

## Key Fields/Parameters

- `data-qa-mode`: `off`, `auto` (default; triggered by a query-shape heuristic, for example "does `<subject>` know X"), or `always`
- `data-qa-subject`: the subject name used by the query-shape heuristic (for example `"does <subject> know X"`)
- source: the same retrieval hits already fetched for search (chunks, and the optional `qa`/`claims` index sections built by `--qa`/`--claims`), scored and selected extractively — no LLM, no model download
- this is independent of, and can be enabled without, the in-browser agent (see [0410](0410-qa-mode.md)); the two can coexist, with the agent offered as a deeper follow-up

## Acceptance Criteria

- `data-qa-mode="off"` never blends an inline answer, regardless of query shape.
- `data-qa-mode="auto"` blends an answer only when the query matches the heuristic's factual shape; other queries show plain search results.
- `data-qa-mode="always"` blends an answer (when the retrieval hits support one) for every query.
- The blend never triggers a model download; it renders as soon as retrieval results are available.
- When `--qa`/`--claims` sections aren't present in the index, the blend still works from chunk hits alone, just with less structured evidence to draw on.

## Evidence

- `tests/integration/test_qa_config.js`

## Linked Tickets

- (none yet)
