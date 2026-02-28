# static-agent

Semantic search and simple Q&A for static sites — fully client-side, no server required.

Index your content at build time, embed a widget on your site, and let visitors search and ask questions — all running in their browser via WebAssembly.

## How it works

1. **Build time:** The CLI reads your markdown/HTML content, chunks it, and generates embeddings using a sentence-transformer model. The result is a compact binary index shipped as a static asset.

2. **Runtime:** A WASM module in the browser downloads the same embedding model (from HuggingFace CDN, cached after first use), embeds the visitor's query, and performs cosine similarity search against the pre-built index.

3. **Optional Q&A:** On browsers with WebGPU support, a small language model can synthesize a short answer from the retrieved content. Falls back gracefully to search-only.

## Quick start

### 1. Index your content

```bash
static-agent-cli index --content-dir content/ --output static/static-agent-index.bin
```

### 2. Embed the widget

```html
<script src="/static-agent-widget.js"></script>
```

### 3. Done

Visitors see a floating search button. First search triggers a one-time model download (~23MB), then searches are instant.

## Configuration

Create `static-agent.toml` in your site root (optional — defaults work out of the box):

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

### Embedding model alternatives

The default model (`all-MiniLM-L6-v2`) is Apache 2.0 but was trained on MS MARCO data with non-commercial restrictions. Models are fetched from HuggingFace CDN at runtime — this project does not redistribute model weights. If training data provenance matters to you, consider:

| Model | License | Params |
|-------|---------|--------|
| `BAAI/bge-small-en-v1.5` | MIT | 33M |
| `Snowflake/snowflake-arctic-embed-s` | Apache 2.0 | 33M |
| `nomic-ai/modernbert-embed-base` | Apache 2.0 | 110M |

## GitHub Actions

```yaml
- name: Index content
  run: |
    curl -L https://github.com/jt55401/static-agent/releases/latest/download/static-agent-cli-linux-amd64 -o static-agent-cli
    chmod +x static-agent-cli
    ./static-agent-cli index --content-dir content/ --output public/static-agent-index.bin
```

## Project layout

```
src/           Rust source (CLI + WASM shared core)
requirements/  Requirements-as-code (requirements-skill format)
docs/plans/    Design documents
tickets/       Linked implementation tickets
```

## Requirements

This project uses [requirements-as-code](https://github.com/jt55401/requirements-skill). See [requirements.md](requirements.md) for the full requirements tree.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-only. See [LICENSE](LICENSE).

Copyright (c) 2026 Jason Grey. Project name and branding are not licensed under GPL — see [TRADEMARKS.md](TRADEMARKS.md).

## Support

If you find this project useful, use the GitHub Sponsor button on the repository.
