# 0120 LLM Model Selection

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can choose which LLM to use for Q&A synthesis, or disable Q&A entirely.

## Key Fields/Parameters

- config key: `qa.model` in `static-agent.toml`
- config key: `qa.enabled` (default: `true`)
- config key: `qa.runtime` — `"webllm"` or `"wllama"` (default: `"webllm"`)
- default model: `SmolLM2-1.7B-Instruct` (Apache 2.0, quantized)

## Acceptance Criteria

- Q&A can be disabled entirely via config (`qa.enabled = false`).
- The LLM model is configurable.
- The runtime (WebLLM vs wllama) is configurable.
- Default model is permissively licensed (Apache 2.0 or MIT).

## Evidence

- `tests/integration/test_qa_config.js`

## Linked Tickets

- (none yet)
