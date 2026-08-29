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
| INDEX-REQ-001 | CLI parses markdown and produces BM25 + sparse + dense retrieval arms | `tests/cli/test_indexer.rs` | (none) | Proposed |
| SEARCH-REQ-001 | WASM module embeds queries and returns fused, ranked results | `tests/wasm/test_search.rs` | (none) | Proposed |
| QA-REQ-001 | Optional in-browser agent (WebLLM) answers from retrieved chunks via WebGPU | `tests/integration/test_qa.js` | (none) | Proposed |
| WIDGET-REQ-001 | Floating button + modal with type-ahead search and optional answer blend/agent | `tests/integration/test_widget.js` | (none) | Proposed |
| INTEG-REQ-001 | GitHub Action indexes Hugo content at build time with a pinned, verified CLI version | `.github/workflows/example-hugo.yml` | (none) | Proposed |
| CONFIG-REQ-001 | Dense/sparse model choice, agent model choice, and UI are configurable via CLI flags and `data-*` attributes (no config file) | `docs/guides/hugo.md` | (none) | Proposed |
