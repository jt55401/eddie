# 0200 High-Level Requirements — Search Runtime

[Requirements Home](../0000-README.md)

The WASM search module runs in the browser. It embeds the user's query for
whichever dense lane the device can run, scores the learned sparse arm and
BM25 with no model at all, and fuses all three with weighted reciprocal
rank fusion against the pre-built index.

## Story Index

- [0110 Query Embedding (Dense Lane Selection)](0100-query-embedding/0110-query-embedding-wasm.md)
- [0120 Sparse Query Scoring](0100-query-embedding/0120-sparse-query-scoring.md)
- [0210 Dense Lane Top-K Scoring](0200-vector-search/0210-cosine-similarity-search.md)
- [0220 Hybrid Fusion, Result Ranking, and Snippets](0200-vector-search/0220-result-ranking-snippets.md)
