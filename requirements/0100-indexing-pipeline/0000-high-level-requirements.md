# 0100 High-Level Requirements — Indexing Pipeline

[Requirements Home](../0000-README.md)

The CLI indexer reads content from a static site source directory, chunks
it into embeddable segments, and builds three retrieval arms: a BM25
keyword index, a learned sparse index, and one or more dense embedding
lanes. The result is serialized as a single static index file (format v5).

## Story Index

- [0110 Markdown Content Parsing](0100-content-parsing/0110-markdown-content-parsing.md)
- [0210 Heading and Semantic Chunking](0200-chunking/0210-section-based-chunking.md)
- [0310 Dense Embedding Generation](0300-embedding/0310-embedding-generation.md)
- [0315 Learned Sparse Index Generation](0300-embedding/0315-sparse-index-generation.md)
- [0320 Index Serialization (Format v5)](0300-embedding/0320-index-serialization.md)
