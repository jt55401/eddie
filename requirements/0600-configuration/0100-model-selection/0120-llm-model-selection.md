# 0120 Agent Model Selection

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can choose which model the in-browser agent uses, or
disable the agent entirely, via `<script>` data attributes (or Hugo
`[params.eddie]`).

## Key Fields/Parameters

- `data-agent-mode`: `off` or `auto` (default); `off` hides the Ask affordance entirely regardless of device capability
- `data-agent-model`: `auto` (default, `Qwen3.5-0.8B`), `quality` (`Qwen3.5-2B`), or any other value passed through as a literal WebLLM model id
- runtime: WebLLM only; search-only fallback whenever the device gate (see [0300-qa-runtime](../../0300-qa-runtime/0100-webgpu-detection/0110-webgpu-detection-fallback.md)) fails
- this is distinct from `data-qa-mode`, which controls the non-LLM inline retrieval answer blend (see [0420](../../0400-widget-ui/0400-qa-mode/0420-inline-retrieval-answer-blend.md)) and needs no model at all

## Acceptance Criteria

- The agent can be disabled entirely via `data-agent-mode="off"` (Ask affordance not rendered).
- The agent model is configurable via `data-agent-model`.
- Both built-in choices (`auto`, `quality`) are permissively licensed (Qwen3.5, Apache-2.0-family license).
- Configuration is read from the `<script>` tag's data attributes (same pattern as existing widget config), or from `[params.eddie]` via the Hugo Module partial.

## Evidence

- `tests/integration/test_qa_config.js`

## Linked Tickets

- (none yet)
