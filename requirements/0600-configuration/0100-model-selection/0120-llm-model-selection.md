# 0120 LLM Model Selection

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can choose which LLM to use for Q&A synthesis, or disable Q&A entirely, via `<script>` data attributes.

## Key Fields/Parameters

- `data-qa-enabled` — `"true"` (default) or `"false"` to hide Ask button entirely
- `data-qa-model` — WebLLM model ID (default: `Qwen2.5-0.5B-Instruct-q4f16_1-MLC`, ~350MB)
- runtime: WebLLM only (no wllama — search-only fallback when WebGPU unavailable)
- recommended models: `Qwen2.5-0.5B-Instruct-q4f16_1-MLC` (350MB), `Qwen2.5-1.5B-Instruct-q4f16_1-MLC` (900MB)

## Acceptance Criteria

- Q&A can be disabled entirely via `data-qa-enabled="false"` (Ask button not rendered).
- The LLM model is configurable via `data-qa-model` attribute.
- Default model is permissively licensed (Apache 2.0 or MIT).
- Configuration is read from the `<script>` tag's data attributes (same pattern as existing widget config).

## Evidence

- `tests/integration/test_qa_config.js`

## Linked Tickets

- (none yet)
