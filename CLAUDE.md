# Eddie

Hybrid search (BM25 + learned sparse + dense) and an optional in-browser
agent for static sites. Rust codebase compiling to native CLI + browser
WASM.

## Build Commands

```bash
cargo build                              # Build CLI (native)
cargo build --target wasm32-unknown-unknown --lib  # Build WASM module
cargo test                               # Run tests
python3 .claude/scripts/check_requirements_conflicts.py --root requirements  # Validate requirements
```

`rust-toolchain.toml` pins the exact Rust version CI uses (1.96.0 plus the
`wasm32-unknown-unknown` target); keep local `cargo`/`rustc`/`clippy` on the
same version so `cargo build` and `cargo clippy` never disagree on a shared
`target/`.

## Architecture

- `src/lib.rs`: shared core, re-exports every module below
- `src/main.rs`: CLI (`eddie index`, `search`, `stats`, `eval`, `tune`, `qa-corpus`, `claims-corpus`)
- `src/parse/`: one content parser per CMS (`hugo.rs`, `astro.rs`, `docusaurus.rs`, `eleventy.rs`, `jekyll.rs`, `mkdocs.rs`)
- `src/chunk.rs`: heading and semantic chunking
- `src/embed.rs`: dense embedding inference via Candle (bert, xlm-roberta, qwen3 families; native + WASM)
- `src/sparse.rs`: learned sparse arm (doc-side encoder at index time, IDF-lookup query side, shared native/WASM)
- `src/bm25.rs`: BM25 keyword index and scoring
- `src/index.rs`: binary index format (`SAED`/`SAGI`, format v5; serialize/deserialize/validate)
- `src/manifest.rs`: index manifest (dense lane specs, sparse config, versioning)
- `src/search.rs`: per-arm retrieval, weighted reciprocal rank fusion, page grouping, snippets
- `src/qa.rs`: build-time QA-pair synthesis (heuristic, Ollama, OpenRouter)
- `src/claims.rs`: build-time claim extraction and manual edits
- `src/eval.rs`: Hit@k / MRR / nDCG against labelled queries (`eddie eval`, `eddie tune`)
- `src/wasm.rs`: the entire WASM/JS binding surface (`wasm-bindgen`)

## Key Decisions

- **Candle** for dense embedding inference (both native and WASM); WebGPU dense lanes run in JS via transformers.js instead, since Candle doesn't target WebGPU
- **Models fetched from HuggingFace** at runtime (not bundled)
- **Three retrieval arms** (BM25, learned sparse, dense), fused with reciprocal rank fusion rather than any single ranking signal
- **WebLLM for the in-browser agent**, gated on a WebGPU adapter, with graceful fallback to retrieval-only
- **GPL-3.0-only** license with trademark protections

## Requirements

Requirements-as-code in `requirements/`. See [requirements.md](requirements.md) for navigation.

## Conventions

- SPDX license header on all source files: `// SPDX-License-Identifier: GPL-3.0-only`
- Conventional commits for requirements changes (see `.claude/references/`)
- 4-digit spaced numbering for requirement files
