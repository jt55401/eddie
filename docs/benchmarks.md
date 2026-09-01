<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Benchmarks

How Eddie is measured, and how it compares with other search tools for
static sites.

To measure your own site rather than these corpora, see [tuning](tuning.md).

## Contents

- [Running the suite](#running-the-suite)
- [What it measures](#what-it-measures)
- [How Eddie compares](#how-eddie-compares)
- [Papers and references](#papers-and-references)

## Running the suite

`scripts/benchmark_suite.py` times and scores model and dataset
combinations.

```bash
python3 scripts/benchmark_suite.py prepare
python3 scripts/benchmark_suite.py run --generate-queries
python3 scripts/benchmark_suite.py render-report .bench/results/<run_id>
```

`prepare` caches the benchmark corpora locally through sparse git checkouts.
They are excluded from the repository.

`run` indexes and searches each combination on a clean cache and records the
timings. `--generate-queries` uses OpenRouter to write a stable query set per
dataset, and can judge retrieval quality on a sample.

`render-report` writes a CSV, an optional Parquet file, and a markdown
summary table.

`benchmarks/README.md` has the full option list.

## What it measures

**Speed.** Indexing time and query time, on a clean cache, per model and
dataset.

**Quality.** Hit@k, MRR and nDCG@k against the labels in
`benchmarks/relevance_labels.toml`, which are maintained by hand.

Quality numbers only mean something relative to each other. Compare two runs
on the same dataset, not one run against a number from another project.

## How Eddie compares

| Tool | Runs on | Search | Answers | Server | Cost |
|---|---|---|---|---|---|
| **Eddie** | Visitor's browser | Keywords, learned terms and meaning | Yes, cited | No | Free |
| Pagefind | Visitor's browser | Keywords | No | No | Free |
| Algolia DocSearch | Cloud | Keywords and meaning | No | Yes | Free for open source |
| kapa.ai | Cloud | Meaning | Yes | Yes | Enterprise |
| DocsBot | Cloud | Meaning | Yes | Yes | $16–$416/month |

The trade is straightforward. Cloud tools do the work on their servers, so
the visitor downloads nothing and you pay a bill and send them your content.
Eddie does the work on the visitor's device, so you pay nothing and host
nothing, and the visitor downloads a model once if they want results that
match meaning.

Pagefind is the closest comparison: also client-side, also free, keyword
only. If keyword search is enough for your site, it is a smaller download.

## Papers and references

Retrieval:

- [The Probabilistic Relevance Framework: BM25 and Beyond](https://www.nowpublishers.com/article/Details/INR-019)
- [Reciprocal Rank Fusion Outperforms Condorcet and Individual Rank Learning Methods](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf)

Models:

- [BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding](https://arxiv.org/abs/1810.04805)
- [Sentence-BERT: Sentence Embeddings using Siamese BERT-Networks](https://arxiv.org/abs/1908.10084)
- [MiniLM: Deep Self-Attention Distillation for Task-Agnostic Compression of Pre-Trained Transformers](https://arxiv.org/abs/2002.10957)
- [BGE M3-Embedding: Multi-Lingual, Multi-Functionality, Multi-Granularity Text Embeddings](https://arxiv.org/abs/2402.03216)

Runtime:

- [WebAssembly](https://webassembly.org/)
- [WebGPU](https://www.w3.org/TR/webgpu/)
- [Candle](https://github.com/huggingface/candle) — the Rust ML framework Eddie uses on the CPU
- [transformers.js](https://github.com/huggingface/transformers.js) — WebGPU search models
- [WebLLM](https://github.com/mlc-ai/web-llm) — WebGPU answer models

## Related documents

- [Tuning](tuning.md) — measuring your own site
- [Reference](reference.md) — the models and what they cost
