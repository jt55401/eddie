# Eddie

<p align="center">
  <img src="assets/eddie-header.png" alt="Eddie — Your site's shipboard computer" width="400" />
</p>

**Your site's shipboard computer.**

Hybrid semantic + keyword search for static sites, with optional experimental Q&A — fully client-side, no server required. Runs entirely in your visitor's browser via WebAssembly.

> *"I'm just so happy to be doing this for you."*
> — Eddie, the Heart of Gold's shipboard computer

## Don't Panic

Eddie does three things:

1. **Build time:** A CLI reads your markdown/HTML content, chunks it, and generates embeddings using a sentence-transformer model. The result is a compact binary index shipped as a static asset. Simple, elegant, like a fjord.

2. **Runtime:** A WASM module in the browser downloads the same embedding model (from HuggingFace CDN, cached after first use), embeds the visitor's query, and performs hybrid semantic + keyword search against the pre-built index.

3. **Optional Q&A (experimental):** On browsers with WebGPU support, a small language model can synthesize a short answer from retrieved content. This is still experimental and falls back gracefully to search-only on browsers without WebGPU.

## Quick Start

### 1. Index your content

```bash
eddie index --content-dir content/ --output static/eddie-index.bin
```

### 2. Embed the widget

```html
<script src="/eddie-widget.js"></script>
```

### 3. Share and Enjoy

Visitors see a floating search button. First search triggers a one-time model download (~23MB), then searches are instant. The answer to how long subsequent queries take is not 42 — it's closer to 42 milliseconds.

## How It Compares

Eddie is built around fast hybrid retrieval (semantic + BM25) with snippets and ranking. Q&A is included as an optional experimental layer on top.

| Tool | Deployment | Search | Q&A | Server | Cost |
|------|-----------|--------|-----|--------|------|
| **Eddie** | Client (WASM) | Hybrid semantic + BM25 | Experimental (WebGPU) | No | Free |
| Pagefind | Client (WASM) | Keyword | No | No | Free |
| Algolia DocSearch | Cloud | Keyword + neural | No | Yes | Free for OSS |
| kapa.ai | Cloud | Semantic (RAG) | Yes | Yes | Enterprise |
| DocsBot | Cloud | Semantic (RAG) | Yes | Yes | $16–$416/mo |

## Configuration

Create `eddie.toml` in your site root (optional — defaults are carefully chosen, unlike Marvin's personality):

```toml
[embedding]
model = "sentence-transformers/all-MiniLM-L6-v2"

[qa]
enabled = true
runtime = "webllm"
model = "HuggingFaceTB/SmolLM2-1.7B-Instruct"

[widget]
theme = "auto"
position = "bottom-right"
```

### Embedding Model Alternatives

The default model (`all-MiniLM-L6-v2`) is Apache 2.0 but was trained on MS MARCO data with non-commercial restrictions. Models are fetched from HuggingFace CDN at runtime — Eddie doesn't redistribute model weights. If training data provenance matters to you:

| Model | License | Params |
|-------|---------|--------|
| `BAAI/bge-small-en-v1.5` | MIT | 33M |
| `Snowflake/snowflake-arctic-embed-s` | Apache 2.0 | 33M |
| `nomic-ai/modernbert-embed-base` | Apache 2.0 | 110M |

## How It Works

Eddie is a single Rust codebase that compiles to two targets — one might say it's *improbably* versatile:

1. **Native CLI** (`eddie`) — runs at build time to index your content
2. **WASM module** — runs in the browser for search and embedding

The indexing pipeline:

```
Markdown/HTML → parse → chunk → embed (MiniLM, 384-dim) → BM25 index → serialize → index.bin
```

The search pipeline (browser):

```
Query → download model (first use) → embed query → cosine similarity + BM25 → RRF fusion → ranked results
```

ML inference uses [Candle](https://github.com/huggingface/candle) (HuggingFace's Rust ML framework), which compiles to WASM without complaint. This is neural network inference running in a browser to search a blog — far more intelligent than the task demands, which Eddie would tell you is *exactly how he likes it*.

## Q&A Status (Experimental)

The core product value today is hybrid retrieval and result summaries/snippets. Q&A is a best-effort experimental mode and quality is highly dependent on your corpus and client hardware.

What this means in practice:

1. Smaller browser models can produce useful summaries, but factual precision can drift.
2. Broader or inconsistent corpora increase hallucination and contradiction risk.
3. On weaker devices, latency can be too high for a good UX.

Suggested model range to try (if supported by your chosen runtime/toolchain):

- `HuggingFaceTB/SmolLM2-1.7B-Instruct` (current default)
- `Qwen/Qwen2.5-1.5B-Instruct`
- `microsoft/Phi-3.5-mini-instruct`

Corpus strategies that generally improve answer quality:

- Keep content focused by domain/use-case rather than mixing unrelated material.
- Prefer explicit, factual writing with clear headings and stable terminology.
- Add canonical FAQ and glossary pages for key entities, policies, and definitions.
- Keep time-sensitive pages date-stamped and archive/redirect stale copies.
- Avoid indexing low-signal pages (thin marketing copy, duplicate boilerplate).

Likely future direction:

1. Use larger LLMs at index/build time (offline) to generate structured QA artifacts.
2. Produce question-answer pairs grounded in source chunks.
3. Build entity/fact cards with citations and claim-to-source maps.
4. Keep browser runtime lightweight for retrieval and synthesis over precomputed evidence.

This should improve factual reliability without requiring large on-device models for every query, while browser hardware catches up for stronger local Q&A.

## Papers and References

Foundational reading and implementation references behind the current approach:

- [BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding](https://arxiv.org/abs/1810.04805)
- [Sentence-BERT: Sentence Embeddings using Siamese BERT-Networks](https://arxiv.org/abs/1908.10084)
- [MiniLM: Deep Self-Attention Distillation for Task-Agnostic Compression of Pre-Trained Transformers](https://arxiv.org/abs/2002.10957)
- [Reciprocal Rank Fusion Outperforms Condorcet and Individual Rank Learning Methods](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf)
- [The Probabilistic Relevance Framework: BM25 and Beyond](https://www.nowpublishers.com/article/Details/INR-019)
- [WebAssembly](https://webassembly.org/)
- [WebGPU](https://www.w3.org/TR/webgpu/)
- [Hugging Face Candle](https://github.com/huggingface/candle)

## GitHub Actions

```yaml
- name: Index content
  run: |
    curl -L https://github.com/jt55401/eddie/releases/latest/download/eddie-linux-amd64 -o eddie
    chmod +x eddie
    ./eddie index --content-dir content/ --output public/eddie-index.bin
```

## Project Layout

```
src/           Rust source (CLI + WASM shared core)
requirements/  Requirements-as-code
docs/plans/    Design documents
```

## Requirements

This project uses [requirements-as-code](https://github.com/jt55401/requirements-skill). See [requirements.md](requirements.md) for the full requirements tree.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests welcome — just don't ask Eddie to be less cheerful about it.

## License

GPL-3.0-only. See [LICENSE](LICENSE).

Copyright (c) 2026 Jason Grey. Project name and branding are not licensed under GPL — see [TRADEMARKS.md](TRADEMARKS.md).

## Support

If you find Eddie useful, use the GitHub Sponsor button on the repository.

For commercial integration or support, [Improbability Engineers](https://improbabilityengineers.com) offers consulting — they built the ship, after all.

---

*Eddie is the [Heart of Gold](https://en.wikipedia.org/wiki/Heart_of_Gold_(The_Hitchhiker%27s_Guide_to_the_Galaxy)) shipboard computer from The Hitchhiker's Guide to the Galaxy. The Heart of Gold is powered by the Infinite Improbability Drive. [Improbability Engineers](https://improbabilityengineers.com) builds the ship's computer.*

*So long, and thanks for all the search results.*
