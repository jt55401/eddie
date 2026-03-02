# Hugo Integration Guide

This guide covers installing `eddie`, indexing your Hugo site content, and searching from the command line.

> The browser widget (WASM + search UI) is not yet implemented. This guide covers the CLI indexer only.

## Prerequisites

- Rust toolchain (1.70+): https://rustup.rs
- A Hugo site with markdown content using TOML (`+++`) or YAML (`---`) frontmatter

## Installation

### From source

```bash
git clone https://github.com/jt55401/eddie.git
cd eddie
cargo build --release
```

The binary is at `target/release/eddie`.

Optionally copy it somewhere on your `$PATH`:

```bash
cp target/release/eddie ~/.local/bin/
```

## Indexing your site

Run the indexer against your Hugo `content/` directory:

```bash
eddie index \
  --content-dir /path/to/your-hugo-site/content/ \
  --output eddie-index.bin
```

This will:

1. Walk the content directory and parse all `.md` files
2. Skip drafts (`draft = true`) and unpublished files (`published = false`)
3. Parse TOML and YAML frontmatter for metadata (title, date, tags, description)
4. Strip Hugo shortcodes (`rawhtml`, `ref`, `certimage`, `mermaid`, `closing`, and any others)
5. Strip markdown formatting to produce clean text
6. Split content into chunks by section headings, with paragraph/sentence splitting for long sections
7. Generate 384-dimensional embeddings using the `all-MiniLM-L6-v2` model (downloaded automatically from HuggingFace Hub on first run, ~23MB, cached)
8. Build a BM25 keyword index alongside the semantic embeddings
9. Write a single binary index file

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--content-dir` | (required) | Path to your Hugo `content/` directory |
| `--output` | `index.bin` | Output path for the index file |
| `--model` | `sentence-transformers/all-MiniLM-L6-v2` | HuggingFace model ID |
| `--chunk-size` | `256` | Maximum tokens per chunk |
| `--overlap` | `32` | Overlap tokens between consecutive chunks |

### Hugo build integration

To index as part of your Hugo build:

```bash
hugo && eddie index \
  --content-dir content/ \
  --output public/eddie-index.bin
```

Or place the index in Hugo's `static/` directory so it's included automatically:

```bash
eddie index \
  --content-dir content/ \
  --output static/eddie-index.bin

hugo  # copies static/ contents to public/
```

### GitHub Actions

```yaml
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-action/setup@v1

      - name: Build eddie
        run: |
          git clone https://github.com/jt55401/eddie.git /tmp/eddie
          cd /tmp/eddie && cargo build --release
          cp target/release/eddie /usr/local/bin/

      - name: Index content
        run: eddie index --content-dir content/ --output static/eddie-index.bin

      - name: Build Hugo site
        run: hugo

      - name: Deploy
        # your deployment step here
```

## Searching

### Hybrid search (default)

Combines semantic similarity with BM25 keyword matching using reciprocal rank fusion:

```bash
eddie search \
  --index eddie-index.bin \
  --query "What programming languages does Jason know?" \
  --top-k 5
```

### Semantic search only

Uses embedding cosine similarity — good for meaning-based queries:

```bash
eddie search \
  --index eddie-index.bin \
  --query "enterprise web development" \
  --mode semantic
```

### Keyword search only

Uses BM25 scoring — good for exact term matching:

```bash
eddie search \
  --index eddie-index.bin \
  --query "Azure certifications" \
  --mode keyword
```

### Search options

| Flag | Default | Description |
|------|---------|-------------|
| `--index` | (required) | Path to the index file |
| `--query` | (required) | Search query text |
| `--top-k` | `5` | Number of results to return |
| `--model` | `sentence-transformers/all-MiniLM-L6-v2` | Must match the model used during indexing |
| `--mode` | `hybrid` | Search mode: `semantic`, `keyword`, or `hybrid` |

## What gets indexed

- All `.md` and `.markdown` files in the content directory (recursively)
- Files with `draft = true` or `published = false` in frontmatter are skipped
- Empty files (after stripping) are skipped
- Hugo shortcodes are removed before indexing
- Markdown formatting is stripped, keeping readable text

### URL derivation

URLs are derived from file paths relative to the content root:

| File path | Derived URL |
|-----------|-------------|
| `content/posts/my-post.md` | `/posts/my-post/` |
| `content/about/index.md` | `/about/` |
| `content/posts/_index.md` | `/posts/` |

## Alternative embedding models

The default model works well for general English content. For different needs:

| Model | License | Dimensions | Notes |
|-------|---------|------------|-------|
| `sentence-transformers/all-MiniLM-L6-v2` | Apache 2.0 | 384 | Default, good balance of speed and quality |
| `BAAI/bge-small-en-v1.5` | MIT | 384 | MIT licensed alternative |
| `Snowflake/snowflake-arctic-embed-s` | Apache 2.0 | 384 | Clean training data provenance |

To use a different model, pass `--model` to both `index` and `search`:

```bash
eddie index --content-dir content/ --output index.bin \
  --model BAAI/bge-small-en-v1.5

eddie search --index index.bin --query "test" \
  --model BAAI/bge-small-en-v1.5
```

## Troubleshooting

### First run is slow

The embedding model (~23MB) is downloaded from HuggingFace Hub on first run and cached in `~/.cache/huggingface/`. Subsequent runs use the cache.

### Model mismatch errors

The `--model` flag during search must match the model used during indexing. The model ID is stored in the index file and checked at load time.

### No results for a query

- Check that the content directory path is correct
- Verify files aren't all marked as drafts
- Try `--mode keyword` to test if the content was indexed
- Try broader queries — semantic search works on meaning, not exact words
