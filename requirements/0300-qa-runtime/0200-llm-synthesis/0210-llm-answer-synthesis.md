# 0210 LLM Answer Synthesis

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor with WebGPU, I can click "Ask" (or press Shift+Enter) after a search and get a concise answer synthesized from the most relevant content, with links to the source pages.

## Key Fields/Parameters

- runtime: WebLLM (`@mlc-ai/web-llm`) — uses TVM-compiled WebGPU shaders for LLM inference
- model: configurable via `data-qa-model` attribute (default: `Qwen2.5-0.5B-Instruct-q4f16_1-MLC`, ~350MB)
- model source: fetched from HuggingFace CDN on first Ask use (not on page load)
- context: top-k retrieved chunks (already available from search) injected as prompt context
- output: short answer (1-3 sentences) + source page links
- streaming: answer tokens streamed to UI as generated

## Acceptance Criteria

- LLM model downloads on first Ask use (not on page load or modal open).
- Download progress is shown to the user with percentage and size.
- The generated answer cites which page(s) it drew from via a "Sources" section.
- Answers are concise (1-3 sentences), grounded in the retrieved chunks only.
- Model weights are cached in browser (Cache API / IndexedDB) after first download.
- Answer is streamed token-by-token as the LLM generates it.
- No Rust/WASM changes — LLM inference is handled entirely in JavaScript via WebLLM.

## Evidence

- `tests/integration/test_qa_synthesis.js`

## Linked Tickets

- (none yet)
