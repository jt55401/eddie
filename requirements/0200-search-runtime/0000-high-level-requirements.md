# 0200 High-Level Requirements — Search Runtime

[Requirements Home](../0000-README.md)

The WASM search module runs in the browser. It downloads a sentence-transformer model on first use, embeds the user's query, and performs cosine similarity search against the pre-built index.

## Story Index

- [0110 Query Embedding in WASM](0100-query-embedding/0110-query-embedding-wasm.md)
- [0210 Cosine Similarity Search](0200-vector-search/0210-cosine-similarity-search.md)
- [0220 Result Ranking and Snippets](0200-vector-search/0220-result-ranking-snippets.md)
