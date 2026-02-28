# 0210 LLM Answer Synthesis

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor with WebGPU, I can ask a question and get a 1-2 sentence answer synthesized from the most relevant content, with links to the source pages.

## Key Fields/Parameters

- runtime: WebLLM or wllama (configurable)
- model: configurable (default: SmolLM2-1.7B-Instruct or Phi-4-mini)
- model source: fetched from HuggingFace CDN on first Q&A use
- context: top-k retrieved chunks injected as prompt context
- output: short answer (1-3 sentences) + source page links

## Acceptance Criteria

- LLM model downloads on first Q&A use (not on page load).
- Download progress is shown to the user.
- The generated answer cites which page(s) it drew from.
- Answers are concise (1-3 sentences), not open-ended conversation.
- Model weights are cached in browser after first download.

## Evidence

- `tests/integration/test_qa_synthesis.js`

## Linked Tickets

- (none yet)
