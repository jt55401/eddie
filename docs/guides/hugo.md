# Hugo Integration Guide

This guide covers installing `eddie`, indexing your Hugo site content, and
embedding the in-browser search widget. See [README.md](../../README.md) for
the full retrieval architecture, the model table, and the in-browser agent.

## Prerequisites

- A Hugo site with markdown content using TOML (`+++`) or YAML (`---`) frontmatter, Hugo 0.110.0+
- Either: Node.js 18+, Ruby 3+, or Python 3.9+ (to run the CLI via a launcher package), or a Rust toolchain (1.96.0, see `rust-toolchain.toml`) to build from source

## Installing the widget

The [`eddie-hugo` Hugo Module](https://github.com/jt55401/eddie-hugo) is the
turnkey path: it ships the widget bundle, a partial that renders the
`<script>` tag with every `data-*` attribute wired to `[params.eddie]` in
your `hugo.toml`, and a CLI wrapper script.

```toml
# go.mod
require github.com/jt55401/eddie-hugo v0.4.0
```

```toml
# hugo.toml
[module]
  [[module.imports]]
    path = "github.com/jt55401/eddie-hugo"
```

Call the partial once, near `</head>`, in your theme's base template:

```go-html-template
{{ partial "eddie/inject.html" . }}
```

Configure it under `[params.eddie]` (all keys optional; see
`hugo-module/hugo.toml` for the full list and defaults, including
`agentMode`, `agentModel`, `denseRuntime`, and `consentText`):

```toml
[params.eddie]
  indexUrl = "/eddie/index.ed"
  position = "bottom-right"
  theme = "auto"
  qaMode = "auto"
```

If you'd rather not add a Go module dependency, install the npm package
instead. `npx @jt55401/eddie-hugo-install <site-dir>` copies the widget
runtime into `static/eddie/` and wires the `<script>` tag into
`layouts/_default/baseof.html` for you (see
`integrations/hugo/README.md`).

## Indexing your site

Run the indexer against your Hugo `content/` directory:

```bash
eddie index \
  --content-dir /path/to/your-hugo-site/content/ \
  --cms hugo \
  --output static/eddie/index.ed \
  --preset balanced
```

This will:

1. Walk the content directory and parse all `.md` files, skipping drafts (`draft = true`) and unpublished files (`published = false`)
2. Parse TOML and YAML frontmatter for metadata (title, date, tags, description)
3. Chunk content by heading (`--chunk-strategy heading`, the default) or by embedding-driven semantic boundaries (`--chunk-strategy semantic`)
4. Build three retrieval arms: BM25 keyword, a learned sparse arm (`--sparse` or `--sparse-model`), and one or more dense embedding lanes (`--dense-model`, repeatable; the default is the bundled `sentence-transformers/multi-qa-MiniLM-L6-cos-v1` bert-family model)
5. Write a single Brotli-compressed index (`.ed`, format v5)

### Presets

`--preset` bundles a dense model set (and device) in one flag:

| Preset | Dense lane(s) | Sparse | Device |
|---|---|---|---|
| `fast` | MiniLM-L6 | no | CPU |
| `balanced` | bge-small-en-v1.5 | yes | CPU |
| `quality` | bge-small-en-v1.5 + Qwen3-Embedding-0.6B | yes | CPU |
| `gpu` | same as `quality` | yes | CUDA |

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--content-dir` | (required) | Path to your Hugo `content/` directory |
| `--cms` | (required) | Content format; use `hugo` |
| `--output` | `index.ed` | Output path for the index file |
| `--preset` | (none) | `fast`, `balanced`, `quality`, or `gpu`; see above; overrides `--dense-model`/`--sparse`/`--device` |
| `--dense-model` | `sentence-transformers/multi-qa-MiniLM-L6-cos-v1` | Repeatable; one dense retrieval lane per flag |
| `--sparse` | off | Enable the learned sparse arm with its default model |
| `--sparse-model` | (none) | Enable the sparse arm with an explicit HuggingFace model id |
| `--device` | `auto` | `auto`, `cpu`, or `cuda` (CUDA requires a build with `--features cuda`) |
| `--batch-size` | `32` | Embedding batch size |
| `--chunk-size` | `256` | Target chunk size, in the dense lane's tokenizer wordpieces |
| `--overlap` | `32` | Overlap tokens between consecutive chunks |
| `--chunk-strategy` | `heading` | `heading` or `semantic` |
| `--qa`, `--claims` | off | Optional build-time QA/claims synthesis lanes (see README) |

`eddie index` reports the per-lane truncation count (chunks that hit
`--chunk-size` at that lane's tokenizer) so you can tell whether a smaller
chunk size or a different lane is warranted.

### Hugo build integration

Place the index in Hugo's `static/` directory so it's included automatically:

```bash
eddie index --content-dir content/ --cms hugo --output static/eddie/index.ed --preset balanced

hugo  # copies static/ contents to public/
```

### GitHub Actions

Use `.github/workflows/example-hugo.yml` from this repo as a starting point.
It pins both the Hugo version and the `@jt55401/eddie-cli` version
explicitly (never `latest`), and the CLI launcher verifies the downloaded
binary against `SHA256SUMS` before running it. See
[docs/guides/github-actions.md](github-actions.md) for the full pipeline and
why pinning matters here.

## Searching

```bash
eddie search --index static/eddie/index.ed --query "What programming languages does Jason know?" --top-k 5
```

`eddie search` reads the dense lane(s), the sparse arm, and the tokenizer
straight from the index; there's no `--model` flag, and no way for the
query-time model to drift from the one used at index time.

### Search modes

| `--mode` | Arms used |
|---|---|
| `hybrid` (default) | BM25 + sparse + dense, fused with weighted reciprocal rank fusion |
| `dense` | The dense lane(s) only, meaning-based |
| `sparse` | The learned sparse arm only |
| `keyword` | BM25 only, exact term matching |

Pass `--lane <id>` to pick a specific dense lane when an index has more than
one (see `eddie stats --index <path>` for the lane ids), and `--json` for
machine-readable output.

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

## Dense lanes and the sparse arm

The dense retrieval lane runs one of two ways at query time: `wasm-candle`
(bert-family models, always available, runs on CPU in the WASM module) or
`webgpu-onnx` (larger models via transformers.js, only when the visitor's
browser gives a WebGPU adapter). See README.md's model table for which
models fit which lane, and their license and download size.

The sparse arm needs no runtime model at query time; it tokenizes the query
and looks up per-term IDF weights stored in the index, so it costs nothing
extra in the browser once the index is loaded.

## Index format

Indexes use format v5 (`SAED`/`SAGI` containers). A v0.4 CLI rejects v1-v4
index files with a "rebuild with eddie 0.4" message. Rebuild with
`eddie index` after upgrading; there's no in-place migration.

## Troubleshooting

### First run is slow

Model weights are downloaded from HuggingFace Hub/CDN on first use and
cached (`~/.cache/huggingface/` for the CLI, IndexedDB for the browser
runtime). Subsequent runs use the cache.

### "rebuild with eddie 0.4" error loading an index

The index was built with an older Eddie version (format v1-v4). Rebuild it:
`eddie index --content-dir content/ --cms hugo --output static/eddie/index.ed`.

### No results for a query

- Check that the content directory path is correct
- Verify files aren't all marked as drafts
- Try `--mode keyword` to test if the content was indexed
- Try broader queries. The dense and sparse arms work on meaning and learned term weights, not just exact words
