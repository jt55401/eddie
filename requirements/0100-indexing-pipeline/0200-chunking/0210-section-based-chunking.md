# 0210 Section-Based Chunking

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As an indexer, I split parsed content into chunks sized appropriately for embedding models, preserving section boundaries where possible.

## Key Fields/Parameters

- default chunk size: ~256 tokens (configurable)
- overlap: ~32 tokens between adjacent chunks
- boundary preference: split at headings, paragraphs, then sentences
- each chunk retains: source file path, page title, page URL, section heading, chunk index

## Acceptance Criteria

- Chunks respect section boundaries (headings) when possible.
- No chunk exceeds the model's max sequence length.
- Metadata (page URL, title, section) is attached to every chunk.
- Short pages produce a single chunk rather than being split unnecessarily.

## Evidence

- `tests/cli/test_chunking.rs`

## Linked Tickets

- (none yet)
