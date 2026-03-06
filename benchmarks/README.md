# Benchmark Suite

This directory contains the config and stable query sets for Eddie benchmark runs.

## Goals

- Compare Eddie index/search performance across **3 corpus sizes**.
- Compare quality/performance across a **model matrix**.
- Keep runs repeatable with versioned query files.
- Emit analysis-friendly output (`CSV`, optional `Parquet`, markdown report).

## Default Corpora

- `fastapi_docs` (small)
- `kubernetes_docs` (medium)
- `azure_docs` (large)

Datasets are cached under `.bench/cache/datasets/` using sparse git checkout and are excluded from git.

## Query Files

Query files live in `benchmarks/queries/*.json`.

Generate queries with a stronger model (stored for consistency):

```bash
export OPENROUTER_API_KEY=...
python3 scripts/benchmark_suite.py generate-queries --dataset fastapi_docs --force
```

Run for all configured datasets:

```bash
python3 scripts/benchmark_suite.py generate-queries
```

## Deterministic Relevance Labels

Human-editable labels live in `benchmarks/relevance_labels.toml`.

Each label binds `dataset` + `query_id` to expected URLs:

- `relevant_urls`: binary relevance set
- `graded_urls`: optional graded relevance (used for `nDCG@k`)
- `weight`: optional per-query weight (reserved for weighted scoring lanes)

The benchmark report now includes deterministic retrieval metrics from this file:

- `Hit@k`
- `MRR`
- `nDCG@k`

## Run Benchmarks

Build the release binary first (benchmarking debug builds will distort timings):

```bash
cargo build --release
```

```bash
export OPENROUTER_API_KEY=...
python3 scripts/benchmark_suite.py run --generate-queries
```

Filter by dataset/model:

```bash
python3 scripts/benchmark_suite.py run \
  --dataset fastapi_docs \
  --model sentence-transformers/all-MiniLM-L6-v2 \
  --runs-per-combo 1 \
  --query-limit 10
```

Incremental update (single new model only, no full rerun):

```bash
python3 scripts/benchmark_suite.py run \
  --dataset fastapi_docs \
  --model sentence-transformers/multi-qa-MiniLM-L6-cos-v1 \
  --runs-per-combo 1
```

```bash
python3 scripts/benchmark_suite.py run \
  --dataset fastapi_docs \
  --model BAAI/bge-base-en-v1.5 \
  --runs-per-combo 1
```

## Outputs

Each run writes to `.bench/results/<run_id>/`:

- `run_manifest.json`
- `index_runs.csv` (+ optional `index_runs.parquet`)
- `search_results.csv` (+ optional `search_results.parquet`)
- `judgments.csv` (+ optional `judgments.parquet`)
- `benchmark_report.md`

`search_results.csv` includes per-query deterministic fields when labels exist:

- `relevance_labeled`
- `first_relevant_rank`
- `mrr`
- `hit_at_1`, `hit_at_3`, `hit_at_5`
- `ndcg_at_5`

Render/re-render report:

```bash
python3 scripts/benchmark_suite.py render-report .bench/results/<run_id>
```
