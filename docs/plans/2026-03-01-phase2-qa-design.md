# Phase 2: Q&A via WebLLM — Design

**Date:** 2026-03-01
**Author:** Jason Grey
**Status:** Approved

## Problem

Phase 1 delivers instant semantic + keyword search in the browser. But users sometimes want a synthesized answer rather than a list of links. Phase 2 adds an "Ask" button that generates a concise AI answer from the search results — entirely client-side via WebGPU.

## Solution

Layer WebLLM (JavaScript library with TVM-compiled WebGPU shaders) on top of the existing search widget. No Rust/WASM changes needed. The worker orchestrates both: WASM for search, WebLLM for generation.

## Architecture

```
User types query
  → search-as-you-type via WASM (existing, instant)
  → results displayed (existing)

User clicks Ask / Shift+Enter
  → Worker grabs top-5 chunk texts from current search results
  → Worker constructs RAG prompt (system + context chunks + question)
  → Worker calls WebLLM engine.chat.completions.create({ stream: true })
  → Tokens posted to main thread via postMessage
  → Widget renders answer card above results with streaming text
  → Final answer includes "Sources:" with clickable links
```

### Key decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| LLM runtime | WebLLM (`@mlc-ai/web-llm`) | Purpose-built for browser LLM inference. Optimized WebGPU shaders via TVM. OpenAI-compatible streaming API. Built-in model caching. |
| Default model | Qwen2.5-0.5B-Instruct-q4f16_1-MLC (~350MB) | Good quality/size tradeoff for default. Fast download. Configurable up to 1.5B+. |
| Fallback | Search-only | No WebGPU = no Ask button. Search works everywhere. No wllama WASM fallback (complexity not worth it). |
| UI pattern | Ask button next to search input | No tabs or mode switching. Search is always instant. Ask is an explicit action. |
| Keyboard shortcut | Shift+Enter | Natural "enhanced Enter". Enter = search, Shift+Enter = ask. |
| Answer placement | Card above results | Results remain visible below for browsing. |
| Rust changes | None | LLM inference handled entirely in JavaScript. Existing WASM search engine untouched. |

## UI Design

### Input row

```
┌─────────────────────────┐ ┌─────┐
│ search query            │ │ Ask │
└─────────────────────────┘ └─────┘
```

Ask button only rendered when WebGPU detected AND `data-qa-enabled` is not `"false"`.

### Answer card states

1. **Hidden** — default, no Ask triggered yet
2. **Downloading model** — progress bar with percentage and file size
3. **Generating** — streaming tokens with pulsing cursor
4. **Complete** — full answer + "Sources: [Title1] [Title2] [Title3]" as clickable links
5. **Error** — graceful message ("Couldn't generate an answer.")

### Ask button states

- **Ready** — clickable (magnifying glass or sparkle icon)
- **Loading** — spinner (during model download or generation)
- **Disabled** — while already generating

### Layout with answer

```
┌─────────────────────────────────┐
│  Search                  ×      │
├─────────────────────────────────┤
│  ┌─────────────────────┐ [Ask]  │
│  │ rust programming    │        │
│  └─────────────────────┘        │
│                                 │
│  ┌─ AI Answer ───────────────┐  │
│  │ Rust is covered in the    │  │
│  │ systems programming       │  │
│  │ series, focusing on...    │  │
│  │                           │  │
│  │ Sources: [1] [2] [3]      │  │
│  └───────────────────────────┘  │
│                                 │
│  ┌─ Getting Started with Rust ┐ │
│  │  Introduction to ownership │ │
│  └────────────────────────────┘ │
│  ┌─ Async Rust Patterns ──────┐ │
│  │  Using tokio for async...  │ │
│  └────────────────────────────┘ │
└─────────────────────────────────┘
```

## Data flow

### WebGPU detection

On first modal open:
1. Check `navigator.gpu` exists
2. Call `navigator.gpu.requestAdapter()` to verify GPU access
3. If both succeed → render Ask button
4. If either fails → no Ask button, no errors

### Model lifecycle

1. Model NOT downloaded until first Ask click
2. On first Ask: download from HuggingFace CDN with streaming progress
3. Cache in browser (Cache API via WebLLM's built-in caching)
4. Subsequent Ask calls: model loads from cache in seconds
5. WebLLM's `ServiceWorkerMLCEngine` could persist model across page navigations (future optimization)

### Prompt template

```
System: You are a helpful assistant for a website. Answer the user's question
based ONLY on the provided search results. Be concise (1-3 sentences). Cite
sources by their number. If the search results don't contain the answer, say so.

User:
Search results:

[1] "Getting Started with Rust" (https://example.com/rust-intro)
Rust is a systems programming language focused on safety and performance...

[2] "Async Rust Patterns" (https://example.com/async-rust)
The tokio runtime provides async I/O for Rust applications...

[3] "Systems Programming Guide" (https://example.com/systems)
Modern systems programming emphasizes memory safety without garbage collection...

Question: What is Rust used for?
```

## Configuration

Via `<script>` data attributes (same pattern as Phase 1):

```html
<script src="/eddie-widget.js"
        data-index-url="/eddie-index.bin"
        data-qa-enabled="true"
        data-qa-model="Qwen2.5-0.5B-Instruct-q4f16_1-MLC"
        defer></script>
```

| Attribute | Default | Description |
|-----------|---------|-------------|
| `data-qa-enabled` | `"true"` | Set to `"false"` to hide Ask button entirely |
| `data-qa-model` | `Qwen2.5-0.5B-Instruct-q4f16_1-MLC` | WebLLM model ID |

### Recommended models

| Model | Size | Quality | Use case |
|-------|------|---------|----------|
| `Qwen2.5-0.5B-Instruct-q4f16_1-MLC` | ~350MB | Good | Default, fast download |
| `Qwen2.5-1.5B-Instruct-q4f16_1-MLC` | ~900MB | Better | Sites wanting higher quality |
| `Llama-3.2-1B-Instruct-q4f16_1-MLC` | ~600MB | Good | Alternative architecture |

## Files changed

| File | Change | Effort |
|------|--------|--------|
| `widget/src/worker.js` | Add WebLLM import, model download, `generate_answer` handler, streaming | Medium |
| `widget/src/eddie-widget.js` | Add Ask button, answer card, Shift+Enter, streaming UI, WebGPU detection | Medium |
| `widget/build.sh` | Copy WebLLM JS to dist/ or reference from CDN | Low |
| No Rust changes | — | Zero |

## What's NOT in scope

- No wllama/WASM fallback
- No server-side anything
- No Rust/Candle LLM code
- No changes to the index format or search engine
- No conversational memory (each Ask is independent)
- No multi-turn chat

## Non-goals

- Full conversational agent
- Server-side inference
- Supporting models >3GB (impractical for widget UX)
