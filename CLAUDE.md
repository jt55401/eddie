# Eddie

Semantic search and simple Q&A for static sites. Rust codebase compiling to native CLI + browser WASM.

## Build Commands

```bash
cargo build                              # Build CLI (native)
cargo build --target wasm32-unknown-unknown --lib  # Build WASM module
cargo test                               # Run tests
python3 .claude/scripts/check_requirements_conflicts.py --root requirements  # Validate requirements
```

## Architecture

- `src/lib.rs` — shared core (chunk, embed, index, search)
- `src/main.rs` — CLI indexer (`eddie`)
- `src/chunk.rs` — content chunking
- `src/embed.rs` — sentence-transformer inference via Candle
- `src/index.rs` — binary index format (serialize/deserialize)
- `src/search.rs` — cosine similarity search

## Key Decisions

- **Candle** for ML inference (both native and WASM)
- **Models fetched from HuggingFace CDN** at runtime (not bundled)
- **Brute-force cosine similarity** (sufficient for <10k chunks)
- **WebGPU for LLM Q&A** with graceful fallback to search-only
- **GPL-3.0-only** license with trademark protections

## Requirements

Requirements-as-code in `requirements/`. See [requirements.md](requirements.md) for navigation.

## Conventions

- SPDX license header on all source files: `// SPDX-License-Identifier: GPL-3.0-only`
- Conventional commits for requirements changes (see `.claude/references/`)
- 4-digit spaced numbering for requirement files
