# 0100 High-Level Requirements — Indexing Pipeline

[Requirements Home](../0000-README.md)

The CLI indexer reads content from a static site source directory, chunks it into embeddable segments, runs a sentence-transformer model to produce vectors, and serializes the result as a static index file.

## Story Index

- [0110 Markdown Content Parsing](0100-content-parsing/0110-markdown-content-parsing.md)
- [0120 HTML Content Parsing](0100-content-parsing/0120-html-content-parsing.md)
- [0210 Section-Based Chunking](0200-chunking/0210-section-based-chunking.md)
- [0310 Embedding Generation](0300-embedding/0310-embedding-generation.md)
- [0320 Index Serialization](0300-embedding/0320-index-serialization.md)
