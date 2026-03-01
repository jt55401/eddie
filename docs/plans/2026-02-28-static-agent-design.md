# static-agent Design

**Date:** 2026-02-28
**Author:** Jason Grey
**Status:** Approved

## Problem

Static sites have no server to run search or answer questions. Existing solutions either require a backend (DocsGPT, Flowise) or a paid service (DocsBot, Kapa.ai). There is no open-source, fully client-side solution that indexes content at build time and runs semantic search + optional Q&A entirely in the browser.

## Solution

A Rust library that compiles to both a native CLI (for build-time indexing) and a WASM module (for browser-side search). A thin JS widget wraps the WASM module and provides the UI.

## Architecture

### Three compilation targets from one Rust codebase

1. **`static-agent-cli`** — native binary, runs at build time in CI
2. **`static-agent-wasm`** — browser module for embeddings + vector search (CPU/WASM)
3. **JS widget** — thin UI layer, handles model download progress, optional LLM integration

### Build time (CI)

```
Markdown/HTML content → parse → chunk → embed (MiniLM via Candle) → serialize → index.bin
```

The CLI reads content from a static site source directory, splits it into chunks, runs a sentence-transformer model to produce embeddings, and outputs a compact binary index.

### Runtime (browser)

```
User query → download model (first use) → embed query → cosine search → ranked results
```

The WASM module loads the same embedding model used at index time (fetched from HuggingFace CDN on first use), embeds the query, and performs brute-force cosine similarity against the pre-built index.

### Two tiers

1. **Search** (CPU/WASM, works everywhere): MiniLM embeddings + vector search → ranked results with page titles, snippets, and links.
2. **Q&A** (WebGPU, when available): Small LLM via WebLLM synthesizes a 1-3 sentence answer from retrieved chunks, streamed token-by-token. "Ask" button next to search input (Shift+Enter shortcut). Falls back to search-only when no WebGPU — Ask button simply not rendered.

### Model delivery

Models are **not redistributed** in the project or site assets. They are fetched from HuggingFace CDN at runtime, on demand:
- Embedding model (~23MB): downloaded on first search
- LLM (~350MB default, configurable up to ~900MB+): downloaded on first Ask use

Both are cached in the browser (IndexedDB/Cache API) after first download.

## Technology choices

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Language | Rust | Single codebase for CLI + WASM. Strong WASM ecosystem. |
| ML framework | Candle (HuggingFace) | Proven BERT WASM examples. Native Rust. HuggingFace ecosystem. |
| Tokenizer | `tokenizers` crate | WASM-compatible. Same tokenizer for CLI and browser. |
| Default embedding model | all-MiniLM-L6-v2 | Small (~22M params), fast, well-known. Configurable. |
| LLM runtime (browser) | WebLLM (`@mlc-ai/web-llm`) | Purpose-built WebGPU LLM inference. TVM-compiled shaders, OpenAI-compatible streaming API, built-in caching. |
| Default LLM | Qwen2.5-0.5B-Instruct-q4f16_1-MLC | ~350MB, Apache 2.0, configurable up to 1.5B+. |
| Vector search | Brute-force cosine | Sufficient for <10k chunks. No ANN index needed. |
| Widget | Vanilla JS + CSS | No framework dependency. Embeddable anywhere. |

## Widget UI

Floating button in the bottom-right corner → expands to a modal panel.

### States

1. **Dormant** — floating search button visible
2. **Open (cold)** — modal open, search field visible, no model loaded
3. **Downloading search model** — progress bar ("Loading search... 12/23 MB")
4. **Searching** — embedding query, computing similarity
5. **Results** — ranked list of pages with titles, snippets, links. If WebGPU available, "Ask" button visible next to search input.
6. **Ask: downloading LLM** — progress bar in answer card above results ("Loading AI model... 120/350 MB")
7. **Ask: generating** — streaming answer tokens with pulsing cursor, above results
8. **Ask: complete** — full answer + "Sources:" with clickable links, results still visible below

### Caching

After first download, models are cached in the browser. Subsequent visits skip the download state entirely.

## Configuration

`static-agent.toml` in the site root:

```toml
[embedding]
model = "sentence-transformers/all-MiniLM-L6-v2"

[qa]
enabled = true
model = "Qwen2.5-0.5B-Instruct-q4f16_1-MLC"  # WebLLM model ID

[widget]
theme = "auto"  # "light", "dark", "auto"
position = "bottom-right"  # or "bottom-left"
```

## Integration

### Hugo

1. Add `static-agent.toml` to site root
2. Run `static-agent-cli index --content-dir content/ --output static/static-agent-index.bin`
3. Add `<script src="/static-agent-widget.js"></script>` to `layouts/_default/baseof.html`

### GitHub Actions

```yaml
- name: Index content
  run: |
    curl -L https://github.com/jt55401/static-agent/releases/latest/download/static-agent-cli-linux-amd64 -o static-agent-cli
    chmod +x static-agent-cli
    ./static-agent-cli index --content-dir content/ --output public/static-agent-index.bin
```

## Licensing

- **Project code:** GPL-3.0-only
- **Models:** Fetched from HuggingFace CDN at runtime (not redistributed)
- **Default embedding model (MiniLM):** Apache 2.0 model weights; training data includes MS MARCO (non-commercial). Users concerned about provenance can configure alternatives (BGE-small, Snowflake Arctic-Embed-S, ModernBERT-Embed).
- **Default LLM (SmolLM2):** Apache 2.0, no known restrictions.

## Non-goals

- Full conversational agent (this is search + simple Q&A)
- Server-side inference
- Real-time indexing (build-time only)
- Supporting non-text content (images, video, audio)
