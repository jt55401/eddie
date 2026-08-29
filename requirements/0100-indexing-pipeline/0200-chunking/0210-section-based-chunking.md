# 0210 Heading and Semantic Chunking

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As an indexer, I split parsed content into chunks sized appropriately for
every dense embedding lane in the index, preserving section boundaries
where possible.

## Key Fields/Parameters

- `--chunk-strategy heading|semantic` (default: `heading`)
- `--chunk-size` (default `256`) and `--overlap` (default `32`): both are
  token budgets measured against each dense lane's own tokenizer, not a
  single fixed count — an index with two dense lanes (for example
  bge-small-en-v1.5 and Qwen3-Embedding-0.6B) may split a chunk differently
  per lane if their wordpiece counts diverge enough to cross the budget
- `heading`: split at headings, then paragraphs, then sentences
- `semantic`: split at embedding-similarity boundaries between adjacent
  sentences/paragraphs rather than structural markers
- each chunk retains: source file path, page title, page URL, section
  heading, chunk index, granularity tag

## Acceptance Criteria

- Chunks respect section boundaries (headings) when `--chunk-strategy heading` is used.
- No chunk exceeds `--chunk-size` in any dense lane's own tokenizer.
- Metadata (page URL, title, section, granularity) is attached to every chunk.
- Short pages produce a single chunk rather than being split unnecessarily.
- `eddie index` reports the truncation count (chunks that hit `--chunk-size`) per dense lane, so an operator can tell whether to lower chunk size or change lanes.

## Evidence

- `tests/cli/test_chunking.rs`

## Linked Tickets

- (none yet)
