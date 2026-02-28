# 0300 High-Level Requirements — Q&A Runtime

[Requirements Home](../0000-README.md)

Optional LLM-powered answer synthesis. When the browser supports WebGPU and the user opts in, a small language model generates a short answer from the retrieved chunks. Falls back gracefully to search-only when WebGPU is unavailable.

## Story Index

- [0110 WebGPU Detection and Fallback](0100-webgpu-detection/0110-webgpu-detection-fallback.md)
- [0210 LLM Answer Synthesis](0200-llm-synthesis/0210-llm-answer-synthesis.md)
