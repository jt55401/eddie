# 0300 High-Level Requirements — Q&A Runtime

[Requirements Home](../0000-README.md)

Optional in-browser agent. When the browser gives a capable WebGPU adapter
and the visitor consents to the model download, a small LLM (WebLLM,
Qwen3.5) runs a bounded tool loop over the same retriever used for search
and streams a cited answer. Falls back gracefully to search-only everywhere
else.

## Story Index

- [0110 WebGPU and Device Gating](0100-webgpu-detection/0110-webgpu-detection-fallback.md)
- [0210 Agent Tool Loop and Answer Synthesis](0200-llm-synthesis/0210-llm-answer-synthesis.md)
