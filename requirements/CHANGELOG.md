# Requirements Changelog

## Unreleased

### Added

- Learned sparse retrieval arm requirements: index-time generation (0100-indexing-pipeline/0315) and query-time scoring (0200-search-runtime/0120).
- Multi-platform release requirement covering the five-platform build matrix and launcher checksum verification (0500-integration/0200-github-actions/0220).
- Model-download consent requirement, split out from download progress (0400-widget-ui/0300-download-progress/0320).
- Inline retrieval answer blend requirement, split out from the (now agent-specific) Ask button story (0400-widget-ui/0400-qa-mode/0420).
- Persistent-engine / tiered service-worker requirement covering `data-persist`, `data-warm`, the lite/dense/gpu service worker tiers, transport fallback to page-side workers, and state reuse across navigation (0400-widget-ui/0500-persistent-runtime/0510).

### Changed

- Indexing, search-runtime, and configuration requirements rewritten for three-arm hybrid retrieval (BM25 + learned sparse + dense), repeatable dense lanes, presets, and index format v5; all `--model`-flag and single-model-index language removed.
- Chunking requirement rewritten for `--chunk-strategy heading|semantic` and per-lane tokenizer token budgets.
- Q&A runtime and widget Ask-button requirements rewritten to describe the actual WebLLM-based in-browser agent (bounded tool loop, citations, consent, streaming, stop button) instead of the earlier SmolLM2/Qwen2.5 spec that was never implemented as written.
- Search modal requirement corrected to debounced type-ahead search (not submit-triggered) with no mode tabs, matching the implemented widget.
- Hugo and GitHub Actions integration requirements updated to remove all `eddie.toml` references (no config file exists) and to require pinned, checksum-verified CLI versions instead of `latest`.
- Embedding model selection requirement updated to remove `nomic-ai/modernbert-embed-base` (architecturally incompatible with the Candle `bert` loader) and to describe the `wasm-candle`/`webgpu-onnx` lane split.
- Hugo integration requirement updated for `eddie-boot.js` as the default widget embed script (`loader = "boot"`), with `eddie-widget.js` as the direct/always-on alternative (`loader = "full"`); the default page-view cost is now the boot script, not the full widget.
- Floating trigger button requirement updated to describe `eddie-boot.js` drawing the button and Ctrl/Cmd+K shortcut on every page view by default, the hand-over to `eddie-widget.js` on first interaction, and the `data-warm` warm-up path for returning visitors.
- Query embedding (dense lane selection) requirement updated to describe the `eddie-lite.wasm`/`eddie-dense.wasm` split: the base module ships with no embedding-model code, and `eddie-dense.wasm` is fetched only after a `wasm-candle` lane is accepted.

### Removed

- HTML content-parsing requirement (0100-indexing-pipeline/0100-content-parsing/0120): described a `--format html` flag and rendered-HTML extraction that never existed and isn't planned; only markdown source parsing via `--cms` is implemented.
