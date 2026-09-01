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

## Search quality against keyword-only search

Eddie's reason to exist is that merging three ways of searching beats any
one of them. Here is that claim measured, on two sites with graded
relevance judgements written by hand.

**Personal site**, 75 pages, 45 questions:

| Mode | Hit@10 | MRR | nDCG@10 |
|---|---:|---:|---:|
| **Hybrid (all three)** | **0.978** | **0.814** | **0.774** |
| Meaning only | 0.956 | 0.811 | 0.750 |
| Learned terms only | 0.956 | 0.804 | 0.745 |
| Keywords only | 0.956 | 0.733 | 0.658 |

**Product site**, 22 pages, 35 questions:

| Mode | Hit@10 | MRR | nDCG@10 |
|---|---:|---:|---:|
| **Hybrid (all three)** | **1.000** | 0.732 | **0.722** |
| Meaning only | 0.914 | 0.711 | 0.674 |
| Learned terms only | 1.000 | **0.788** | 0.725 |
| Keywords only | 0.857 | 0.699 | 0.629 |

Hybrid ranks 18% and 15% better than keyword-only by nDCG@10. On the product
site, keyword-only found no correct page at all for 14% of the questions.

Two honest notes. On the product site, the learned-terms arm alone beats
hybrid on MRR (0.788 against 0.732), so hybrid is not the best setting for
every corpus, which is why `eddie eval --sweep` and
`eddie index --weights` exist. And these are two small sites with
self-written judgements: they show the shape of the difference, not a
league table.

Reproduce them with:

```bash
eddie eval --index index.ed --labels labels.toml --graded --all-modes
```

## How Eddie compares

| Tool | Runs on | Search | Answers | Server | Cost |
|---|---|---|---|---|---|
| **Eddie** | Visitor's browser | Keywords, learned terms and meaning | Yes, in the browser, cited | No | Free, GPL-3.0 |
| [Pagefind](https://pagefind.app/) | Visitor's browser | Keywords | No | No | Free, MIT |
| [Orama](https://github.com/oramasearch/orama) | Visitor's browser | Keywords, vector, hybrid | Yes, through an LLM service | No, for search | Free, Apache-2.0. Cloud is paid |
| [Algolia DocSearch](https://docsearch.algolia.com/) | Cloud | Keywords and meaning | No | Yes | Free for open-source docs |
| [kapa.ai](https://www.kapa.ai/) | Cloud | Meaning | Yes | Yes | Enterprise |
| [DocsBot](https://docsbot.ai/) | Cloud | Meaning | Yes | Yes | Free tier, then $49–$499/month |

Checked 2026-09-01 against each project's own site.

The trade is straightforward. Cloud tools do the work on their servers, so
the visitor downloads nothing, and you pay a bill and send them your
content. The two browser-side tools cost you nothing and host nothing, and
ask the visitor for a download instead.

**Pagefind** is the smallest download by a wide margin, and keyword-only.
Its own site describes a 10,000 page site searching within a 300 kB total
payload. If keyword search answers your visitors' questions, it is the
better choice.

**Orama** is the closest alternative to Eddie: client-side, Apache-2.0,
with full-text, vector and hybrid search, and a plugin that can generate
embeddings in the browser. The difference is scope. Orama is a search
library you build on, and it indexes documents you give it. Eddie is a
finished pipeline: a CLI that reads your markdown or built HTML, chunk and
model selection made for you, a widget with consent prompts and a settings
panel, and answers generated in the browser rather than through a service.
If you want to build the search experience yourself, Orama gives you more
room. If you want search on a Hugo site this afternoon, Eddie does more of
the work.

## Measured against Pagefind and Orama

The comparison table above says what each tool does. This says how they
score on the same content and the same questions:

| Tool | nDCG@10, full questions | nDCG@10, 2-word queries | KB to first result |
|---|---:|---:|---:|
| **Eddie** | **0.774** | **0.644** | 738 |
| Orama, full-text | 0.512 | 0.418 | 254 |
| Pagefind | 0.143 | 0.372 | **113** |

Eddie ranks better on every query shape tested and costs about six times
more to reach a first result than Pagefind. Those are the same trade seen
from two sides: Eddie loads its engine and index once and answers
everything after that from memory, Pagefind fetches a small index chunk per
query.

The full write-up has three query shapes, the browser-measured byte
timeline, generator coverage, and the ways this benchmark is unfair:
[Eddie against Pagefind and Orama](reviews/2026-09-01-competitive-benchmark.md).

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
