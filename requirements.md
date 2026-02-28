# Requirements Register

Detailed per-area requirements live in [requirements/0000-README.md](requirements/0000-README.md).

## Navigation

- [Requirements Architecture](requirements/0000-README.md)
- [Requirements Changelog](requirements/CHANGELOG.md)
- [Indexing Pipeline](requirements/0100-indexing-pipeline/0000-high-level-requirements.md)
- [Search Runtime](requirements/0200-search-runtime/0000-high-level-requirements.md)
- [Q&A Runtime](requirements/0300-qa-runtime/0000-high-level-requirements.md)
- [Widget UI](requirements/0400-widget-ui/0000-high-level-requirements.md)
- [Integration](requirements/0500-integration/0000-high-level-requirements.md)
- [Configuration](requirements/0600-configuration/0000-high-level-requirements.md)

## Sample Register

| Req ID | Requirement | Acceptance Evidence | Linked Tickets | Status |
|---|---|---|---|---|
| INDEX-REQ-001 | CLI parses markdown/HTML and produces chunked embeddings | `tests/cli/test_indexer.rs` | — | Proposed |
| SEARCH-REQ-001 | WASM module embeds queries and returns ranked results | `tests/wasm/test_search.rs` | — | Proposed |
| QA-REQ-001 | Optional LLM synthesis from retrieved chunks via WebGPU | `tests/integration/test_qa.js` | — | Proposed |
| WIDGET-REQ-001 | Floating button + modal with search and Q&A modes | `tests/integration/test_widget.js` | — | Proposed |
| INTEG-REQ-001 | GitHub Action indexes Hugo content at build time | `.github/workflows/index.yml` | — | Proposed |
| CONFIG-REQ-001 | Embedding model, LLM, and UI are configurable | `static-agent.toml` | — | Proposed |
