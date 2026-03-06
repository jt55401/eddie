#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
#
# Eddie benchmark harness:
# - caches common doc corpora via sparse git checkout
# - runs repeatable index/search timing matrix for datasets x models
# - optionally uses OpenRouter for query generation and retrieval judging
# - writes CSV (+ optional Parquet) and markdown summary tables

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "benchmarks" / "benchmark.toml"

TEXT_EXTS = {".md", ".mdx", ".txt", ".rst", ".html", ".adoc"}
TIER_ORDER = {"small": 0, "medium": 1, "large": 2}


@dataclass
class SuiteConfig:
    name: str
    runs_per_combo: int
    output_root: Path
    cache_root: Path
    eddie_bin: Path
    keep_indexes: bool


@dataclass
class DatasetConfig:
    dataset_id: str
    size_tier: str
    source: str
    repo: str
    ref: str
    subdir: str
    query_file: Path


@dataclass
class ModelConfig:
    model_id: str


@dataclass
class DatasetResolved:
    cfg: DatasetConfig
    content_dir: Path
    git_revision: str
    file_count: int
    doc_count: int
    total_bytes: int


@dataclass
class RelevanceLabel:
    dataset_id: str
    query_id: str
    relevant_urls: set[str]
    graded_urls: dict[str, float]
    weight: float
    notes: str


def err(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)


def info(msg: str) -> None:
    print(msg, file=sys.stderr)


def run_cmd(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    capture: bool = False,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    if capture:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd) if cwd else None,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
    else:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd) if cwd else None,
            env=env,
            text=True,
            check=False,
        )
    if check and proc.returncode != 0:
        stdout = proc.stdout or ""
        stderr = proc.stderr or ""
        raise RuntimeError(
            f"command failed ({proc.returncode}): {' '.join(cmd)}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    return proc


def load_config(
    config_path: Path,
) -> tuple[
    SuiteConfig,
    dict[str, Any],
    dict[str, Any],
    dict[str, Any],
    dict[str, Any],
    dict[str, Any],
    list[DatasetConfig],
    list[ModelConfig],
]:
    raw = tomllib.loads(config_path.read_text(encoding="utf-8"))

    suite_raw = raw.get("suite", {})
    suite = SuiteConfig(
        name=str(suite_raw.get("name", "eddie-benchmark")),
        runs_per_combo=int(suite_raw.get("runs_per_combo", 1)),
        output_root=resolve_repo_path(str(suite_raw.get("output_root", ".bench/results"))),
        cache_root=resolve_repo_path(str(suite_raw.get("cache_root", ".bench/cache"))),
        eddie_bin=resolve_repo_path(str(suite_raw.get("eddie_bin", "target/debug/eddie"))),
        keep_indexes=bool(suite_raw.get("keep_indexes", False)),
    )

    index_cfg = dict(raw.get("index", {}))
    search_cfg = dict(raw.get("search", {}))
    judge_cfg = dict(raw.get("judge", {}))
    qgen_cfg = dict(raw.get("query_generation", {}))
    relevance_cfg = dict(raw.get("relevance", {}))
    if "labels_file" in relevance_cfg:
        relevance_cfg["labels_file"] = str(resolve_repo_path(str(relevance_cfg["labels_file"])))

    dataset_rows = raw.get("datasets", [])
    if not dataset_rows:
        raise ValueError("config must include at least one [[datasets]] entry")
    datasets: list[DatasetConfig] = []
    for row in dataset_rows:
        ds = DatasetConfig(
            dataset_id=str(row["id"]),
            size_tier=str(row.get("size_tier", "small")),
            source=str(row.get("source", "git")),
            repo=str(row["repo"]),
            ref=str(row.get("ref", "main")),
            subdir=str(row["subdir"]),
            query_file=resolve_repo_path(
                str(row.get("query_file", f"benchmarks/queries/{row['id']}.json"))
            ),
        )
        datasets.append(ds)

    model_rows = raw.get("models", [])
    if not model_rows:
        raise ValueError("config must include at least one [[models]] entry")
    models: list[ModelConfig] = [ModelConfig(model_id=str(row["id"])) for row in model_rows]
    return suite, index_cfg, search_cfg, judge_cfg, qgen_cfg, relevance_cfg, datasets, models


def resolve_repo_path(value: str) -> Path:
    p = Path(value)
    return p if p.is_absolute() else (REPO_ROOT / p)


def ensure_dirs(*paths: Path) -> None:
    for p in paths:
        p.mkdir(parents=True, exist_ok=True)


def slugify_model(model_id: str) -> str:
    return model_id.replace("/", "__").replace(":", "_").replace("@", "_")


def benchmark_env(cache_root: Path) -> dict[str, str]:
    env = os.environ.copy()
    hf_home = cache_root / "hf"
    env["HF_HOME"] = str(hf_home)
    env["HF_HUB_CACHE"] = str(hf_home / "hub")
    env["HUGGINGFACE_HUB_CACHE"] = str(hf_home / "hub")
    env["TRANSFORMERS_CACHE"] = str(hf_home / "transformers")
    return env


def unique_disk_usage_bytes(root: Path) -> int:
    if not root.exists():
        return 0
    total = 0
    seen: set[tuple[int, int]] = set()
    for path in root.rglob("*"):
        try:
            if not path.is_file():
                continue
            st = path.stat()
        except OSError:
            continue
        inode_key = (int(st.st_dev), int(st.st_ino))
        if inode_key in seen:
            continue
        seen.add(inode_key)
        total += int(st.st_size)
    return total


def model_cache_stats(cache_root: Path, model_id: str) -> dict[str, int]:
    model_key = model_id.replace("/", "--")
    candidate_roots = [
        cache_root / "hf" / "hub",
        Path.home() / ".cache" / "huggingface" / "hub",
    ]
    hub_env = os.environ.get("HF_HUB_CACHE", "").strip()
    if hub_env:
        candidate_roots.insert(0, Path(hub_env))
    hub_env2 = os.environ.get("HUGGINGFACE_HUB_CACHE", "").strip()
    if hub_env2:
        candidate_roots.insert(0, Path(hub_env2))

    repo_dir = None
    for root in candidate_roots:
        cand = root / f"models--{model_key}"
        if cand.exists():
            repo_dir = cand
            break
    if repo_dir is None:
        repo_dir = candidate_roots[0] / f"models--{model_key}"

    cache_bytes = unique_disk_usage_bytes(repo_dir)

    weights_bytes = 0
    tokenizer_bytes = 0
    config_bytes = 0
    if repo_dir.exists():
        for p in repo_dir.rglob("*"):
            if not p.is_file():
                continue
            name = p.name
            try:
                size = int(p.stat().st_size)
            except OSError:
                continue
            if name == "model.safetensors":
                weights_bytes = max(weights_bytes, size)
            elif name == "tokenizer.json":
                tokenizer_bytes = max(tokenizer_bytes, size)
            elif name == "config.json":
                config_bytes = max(config_bytes, size)

    return {
        "model_cache_bytes": cache_bytes,
        "model_weights_bytes": weights_bytes,
        "model_tokenizer_bytes": tokenizer_bytes,
        "model_config_bytes": config_bytes,
    }


def list_text_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix.lower() in TEXT_EXTS:
            files.append(path)
    files.sort()
    return files


def scan_content_stats(content_dir: Path) -> tuple[int, int, int]:
    files = list_text_files(content_dir)
    total_bytes = 0
    for p in files:
        try:
            total_bytes += p.stat().st_size
        except OSError:
            continue
    file_count = len(files)
    return file_count, file_count, total_bytes


def ensure_dataset_cached(ds: DatasetConfig, cache_root: Path, *, refresh: bool = False) -> DatasetResolved:
    if ds.source != "git":
        raise ValueError(f"unsupported dataset source: {ds.source}")

    dataset_root = cache_root / "datasets" / ds.dataset_id
    repo_dir = dataset_root / "repo"

    if refresh and dataset_root.exists():
        shutil.rmtree(dataset_root)
    ensure_dirs(dataset_root)

    if not repo_dir.exists():
        info(f"[dataset:{ds.dataset_id}] cloning {ds.repo}")
        run_cmd(
            [
                "git",
                "clone",
                "--depth",
                "1",
                "--filter=blob:none",
                "--no-checkout",
                ds.repo,
                str(repo_dir),
            ],
            check=True,
        )

    fetched_remote = True
    try:
        info(f"[dataset:{ds.dataset_id}] fetching {ds.ref}")
        run_cmd(
            ["git", "-C", str(repo_dir), "fetch", "--depth", "1", "origin", ds.ref], check=True
        )
        run_cmd(["git", "-C", str(repo_dir), "sparse-checkout", "init", "--cone"], check=True)
        run_cmd(["git", "-C", str(repo_dir), "sparse-checkout", "set", ds.subdir], check=True)
        run_cmd(["git", "-C", str(repo_dir), "checkout", "--force", "FETCH_HEAD"], check=True)
    except Exception as exc:
        fetched_remote = False
        info(
            f"[dataset:{ds.dataset_id}] warning: fetch failed ({exc}). "
            "Falling back to existing cached checkout."
        )
        run_cmd(["git", "-C", str(repo_dir), "sparse-checkout", "init", "--cone"], check=True)
        run_cmd(["git", "-C", str(repo_dir), "sparse-checkout", "set", ds.subdir], check=True)

    rev = (
        run_cmd(["git", "-C", str(repo_dir), "rev-parse", "HEAD"], capture=True, check=True)
        .stdout.strip()
    )
    content_dir = repo_dir / ds.subdir
    if not content_dir.exists():
        raise FileNotFoundError(
            f"dataset content dir does not exist for {ds.dataset_id}: {content_dir}"
        )

    file_count, doc_count, total_bytes = scan_content_stats(content_dir)
    meta = {
        "dataset_id": ds.dataset_id,
        "repo": ds.repo,
        "ref": ds.ref,
        "subdir": ds.subdir,
        "git_revision": rev,
        "file_count": file_count,
        "doc_count": doc_count,
        "total_bytes": total_bytes,
        "fetched_remote": fetched_remote,
        "updated_at_utc": utc_now(),
    }
    (dataset_root / "metadata.json").write_text(
        json.dumps(meta, indent=2, sort_keys=True), encoding="utf-8"
    )
    return DatasetResolved(
        cfg=ds,
        content_dir=content_dir,
        git_revision=rev,
        file_count=file_count,
        doc_count=doc_count,
        total_bytes=total_bytes,
    )


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def deterministic_sample(items: list[Path], count: int) -> list[Path]:
    if len(items) <= count:
        return items
    step = len(items) / float(count)
    out = []
    for i in range(count):
        idx = int(i * step)
        out.append(items[idx])
    return out


def sample_corpus_text(
    content_dir: Path, *, max_files: int, max_chars_per_file: int, seed: int = 42
) -> str:
    files = list_text_files(content_dir)
    if not files:
        return ""
    random.seed(seed)
    sampled = deterministic_sample(files, max_files)
    chunks: list[str] = []
    for p in sampled:
        rel = p.relative_to(content_dir).as_posix()
        try:
            raw = p.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        raw = raw.strip()
        if not raw:
            continue
        excerpt = raw[:max_chars_per_file]
        excerpt = re.sub(r"\s+", " ", excerpt).strip()
        chunks.append(f"FILE: {rel}\nEXCERPT: {excerpt}")
    return "\n\n".join(chunks)


def call_openrouter(
    *,
    endpoint: str,
    api_key: str,
    model: str,
    messages: list[dict[str, str]],
    temperature: float = 0.0,
    max_tokens: int = 1200,
) -> str:
    payload = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "response_format": {"type": "json_object"},
    }
    req = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/jt55401/eddie",
            "X-Title": "Eddie Benchmark Suite",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="ignore")
        raise RuntimeError(f"openrouter HTTP {exc.code}: {body}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"openrouter request failed: {exc}") from exc

    raw = json.loads(body)
    try:
        return str(raw["choices"][0]["message"]["content"])
    except (KeyError, IndexError, TypeError) as exc:
        raise RuntimeError(f"unexpected openrouter response shape: {body}") from exc


def extract_json_object(raw_text: str) -> dict[str, Any]:
    text = raw_text.strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    start = text.find("{")
    end = text.rfind("}")
    if start == -1 or end == -1 or end <= start:
        raise ValueError(f"LLM response is not JSON:\n{text}")
    return json.loads(text[start : end + 1])


def generate_queries_for_dataset(
    *,
    ds: DatasetResolved,
    qgen_cfg: dict[str, Any],
    force: bool,
) -> Path:
    out_path = ds.cfg.query_file
    ensure_dirs(out_path.parent)
    if out_path.exists() and not force:
        info(f"[queries:{ds.cfg.dataset_id}] exists: {out_path}")
        return out_path

    model = str(qgen_cfg.get("model", "openai/gpt-4.1"))
    endpoint = str(
        qgen_cfg.get("endpoint", "https://openrouter.ai/api/v1/chat/completions")
    )
    api_key_env = str(qgen_cfg.get("api_key_env", "OPENROUTER_API_KEY"))
    query_count = int(qgen_cfg.get("queries_per_dataset", 30))
    sample_files = int(qgen_cfg.get("sample_files", 40))
    sample_chars = int(qgen_cfg.get("sample_chars_per_file", 1200))
    temperature = float(qgen_cfg.get("temperature", 0.1))

    api_key = os.getenv(api_key_env, "").strip()
    if not api_key:
        raise RuntimeError(
            f"missing env var {api_key_env}; required for query generation"
        )

    sample = sample_corpus_text(
        ds.content_dir,
        max_files=sample_files,
        max_chars_per_file=sample_chars,
    )
    if not sample:
        raise RuntimeError(f"dataset sample is empty for {ds.cfg.dataset_id}")

    system = (
        "You create high-quality retrieval benchmark queries for technical documentation corpora. "
        "Return strict JSON only."
    )
    user = f"""
Dataset ID: {ds.cfg.dataset_id}
Dataset Tier: {ds.cfg.size_tier}
Corpus Sample:
{sample}

Generate exactly {query_count} realistic search queries users might ask this corpus.
Mix easy/medium/hard and include factual + troubleshooting + conceptual intents.

Return this JSON shape exactly:
{{
  "queries": [
    {{
      "id": "q001",
      "query": "...",
      "intent": "factual|troubleshooting|conceptual|navigation",
      "difficulty": "easy|medium|hard",
      "expected_terms": ["term1", "term2"]
    }}
  ]
}}
"""
    raw = call_openrouter(
        endpoint=endpoint,
        api_key=api_key,
        model=model,
        messages=[{"role": "system", "content": system}, {"role": "user", "content": user}],
        temperature=temperature,
        max_tokens=2500,
    )
    parsed = extract_json_object(raw)
    queries = parsed.get("queries", [])
    if not isinstance(queries, list) or not queries:
        raise RuntimeError(f"query generation returned invalid payload: {parsed}")

    normalized = []
    for i, row in enumerate(queries, start=1):
        query = str(row.get("query", "")).strip()
        if not query:
            continue
        qid = str(row.get("id", f"q{i:03d}")).strip() or f"q{i:03d}"
        expected = row.get("expected_terms", [])
        if not isinstance(expected, list):
            expected = []
        normalized.append(
            {
                "id": qid,
                "query": query,
                "intent": str(row.get("intent", "factual")),
                "difficulty": str(row.get("difficulty", "medium")),
                "expected_terms": [str(x) for x in expected if str(x).strip()],
            }
        )
    if len(normalized) < 5:
        raise RuntimeError(
            f"query generation yielded too few usable queries ({len(normalized)})"
        )

    payload = {
        "dataset_id": ds.cfg.dataset_id,
        "size_tier": ds.cfg.size_tier,
        "generator_model": model,
        "generated_at_utc": utc_now(),
        "source_repo": ds.cfg.repo,
        "source_ref": ds.cfg.ref,
        "source_revision": ds.git_revision,
        "queries": normalized,
    }
    out_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    info(f"[queries:{ds.cfg.dataset_id}] wrote {len(normalized)} queries -> {out_path}")
    return out_path


def load_queries(query_file: Path) -> list[dict[str, Any]]:
    if not query_file.exists():
        raise FileNotFoundError(f"query file not found: {query_file}")
    payload = json.loads(query_file.read_text(encoding="utf-8"))
    queries = payload.get("queries", [])
    if not isinstance(queries, list) or not queries:
        raise ValueError(f"query file invalid/empty: {query_file}")
    out = []
    for i, row in enumerate(queries, start=1):
        query = str(row.get("query", "")).strip()
        if not query:
            continue
        out.append(
            {
                "id": str(row.get("id", f"q{i:03d}")).strip() or f"q{i:03d}",
                "query": query,
                "intent": str(row.get("intent", "")),
                "difficulty": str(row.get("difficulty", "")),
                "expected_terms": row.get("expected_terms", []),
            }
        )
    if not out:
        raise ValueError(f"query file has no usable entries: {query_file}")
    return out


def normalize_url_path(url: str) -> str:
    raw = (url or "").strip()
    if not raw:
        return ""
    parsed = urllib.parse.urlparse(raw)
    path = parsed.path if parsed.scheme or parsed.netloc else raw
    path = path.strip()
    if not path:
        return ""
    if not path.startswith("/"):
        path = "/" + path
    path = re.sub(r"/{2,}", "/", path)
    if path != "/":
        path = path.rstrip("/")
    return path.lower()


def parse_k_values(value: Any, default: list[int]) -> list[int]:
    if isinstance(value, list):
        out = []
        for v in value:
            try:
                iv = int(v)
            except Exception:
                continue
            if iv > 0:
                out.append(iv)
        out = sorted(set(out))
        return out or default
    return default


def load_relevance_labels(labels_file: Path) -> dict[tuple[str, str], RelevanceLabel]:
    if not labels_file.exists():
        info(f"[relevance] labels file not found (skipping): {labels_file}")
        return {}

    raw = tomllib.loads(labels_file.read_text(encoding="utf-8"))
    rows = raw.get("labels", [])
    if not isinstance(rows, list):
        raise ValueError(f"relevance labels file has invalid format: {labels_file}")

    out: dict[tuple[str, str], RelevanceLabel] = {}
    for row in rows:
        if not isinstance(row, dict):
            continue
        dataset_id = str(row.get("dataset", "")).strip()
        query_id = str(row.get("query_id", "")).strip()
        if not dataset_id or not query_id:
            continue

        relevant_raw = row.get("relevant_urls", [])
        relevant_urls = {
            normalize_url_path(str(v))
            for v in (relevant_raw if isinstance(relevant_raw, list) else [])
            if str(v).strip()
        }
        relevant_urls.discard("")

        graded_urls: dict[str, float] = {}
        graded_raw = row.get("graded_urls", {})
        if isinstance(graded_raw, dict):
            for k, v in graded_raw.items():
                nk = normalize_url_path(str(k))
                if not nk:
                    continue
                try:
                    score = float(v)
                except Exception:
                    continue
                if score > 0:
                    graded_urls[nk] = score

        for rel in relevant_urls:
            graded_urls.setdefault(rel, 1.0)

        try:
            weight = float(row.get("weight", 1.0))
        except Exception:
            weight = 1.0
        if weight <= 0:
            weight = 1.0

        label = RelevanceLabel(
            dataset_id=dataset_id,
            query_id=query_id,
            relevant_urls=set(graded_urls.keys()),
            graded_urls=graded_urls,
            weight=weight,
            notes=str(row.get("notes", "")),
        )
        out[(dataset_id, query_id)] = label
    return out


def score_relevance_metrics(
    *,
    results: list[dict[str, Any]],
    label: RelevanceLabel | None,
    k_values: list[int],
    ndcg_k: int,
) -> dict[str, float]:
    metrics: dict[str, float] = {
        "relevance_labeled": 0.0,
        "relevance_label_weight": 0.0,
        "first_relevant_rank": 0.0,
        "mrr": 0.0,
    }
    for k in k_values:
        metrics[f"hit_at_{k}"] = 0.0
    metrics[f"ndcg_at_{ndcg_k}"] = 0.0

    if label is None:
        return metrics

    ranked_urls_raw = [normalize_url_path(str(r.get("url", ""))) for r in results]
    ranked_urls: list[str] = []
    seen_urls: set[str] = set()
    for url in ranked_urls_raw:
        if not url or url in seen_urls:
            continue
        seen_urls.add(url)
        ranked_urls.append(url)
    rel_set = label.relevant_urls
    metrics["relevance_labeled"] = 1.0
    metrics["relevance_label_weight"] = label.weight

    first_rank = 0
    for i, url in enumerate(ranked_urls, start=1):
        if url in rel_set:
            first_rank = i
            break
    if first_rank > 0:
        metrics["first_relevant_rank"] = float(first_rank)
        metrics["mrr"] = 1.0 / float(first_rank)

    for k in k_values:
        hit = any(url in rel_set for url in ranked_urls[:k])
        metrics[f"hit_at_{k}"] = 1.0 if hit else 0.0

    dcg = 0.0
    for i, url in enumerate(ranked_urls[:ndcg_k], start=1):
        rel = label.graded_urls.get(url, 0.0)
        if rel <= 0:
            continue
        dcg += (2.0**rel - 1.0) / math.log2(i + 1.0)

    ideal_rels = sorted((v for v in label.graded_urls.values() if v > 0), reverse=True)
    idcg = 0.0
    for i, rel in enumerate(ideal_rels[:ndcg_k], start=1):
        idcg += (2.0**rel - 1.0) / math.log2(i + 1.0)
    if idcg > 0.0:
        metrics[f"ndcg_at_{ndcg_k}"] = dcg / idcg

    return metrics


def parse_index_summary(stderr_text: str) -> tuple[int | None, int | None, int | None]:
    m = re.search(
        r"Done!\s+Index contains\s+(\d+)\s+chunks,\s+(\d+)\s+qa entries,\s+(\d+)\s+claims\.",
        stderr_text,
    )
    if not m:
        return None, None, None
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def parse_search_output(stdout_text: str) -> list[dict[str, Any]]:
    lines = stdout_text.splitlines()
    out: list[dict[str, Any]] = []
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        m = re.match(r"^(\d+)\.\s+(.+?)\s+[—-]\s+(\S+)\s*$", line)
        if not m:
            i += 1
            continue

        rank = int(m.group(1))
        title = m.group(2).strip()
        url = m.group(3).strip()
        snippet_parts: list[str] = []
        section = ""

        j = i + 1
        while j < len(lines):
            candidate = lines[j].strip()
            if not candidate:
                j += 1
                continue
            if re.match(r"^\d+\.\s+", candidate):
                break
            if candidate.startswith("Section:"):
                section = candidate.removeprefix("Section:").strip()
            elif not candidate.startswith("---") and "results for:" not in candidate:
                snippet_parts.append(candidate)
            j += 1

        snippet = " ".join(snippet_parts).strip()
        if not snippet and section:
            snippet = f"Section: {section}"

        out.append({"rank": rank, "title": title, "url": url, "snippet": snippet})
        i = j
    return out


def query_hash(dataset_id: str, model_id: str, query_id: str, results: list[dict[str, Any]]) -> str:
    h = hashlib.sha256()
    h.update(dataset_id.encode("utf-8"))
    h.update(model_id.encode("utf-8"))
    h.update(query_id.encode("utf-8"))
    h.update(json.dumps(results[:5], sort_keys=True).encode("utf-8"))
    return h.hexdigest()


def load_judge_cache(path: Path) -> dict[str, dict[str, Any]]:
    if not path.exists():
        return {}
    out: dict[str, dict[str, Any]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
            key = str(row["cache_key"])
            out[key] = row
        except Exception:
            continue
    return out


def append_judge_cache(path: Path, row: dict[str, Any]) -> None:
    ensure_dirs(path.parent)
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(row, sort_keys=True) + "\n")


def judge_results(
    *,
    judge_cfg: dict[str, Any],
    dataset_id: str,
    model_id: str,
    query: dict[str, Any],
    results: list[dict[str, Any]],
    cache: dict[str, dict[str, Any]],
    cache_path: Path,
) -> dict[str, Any]:
    cache_key = query_hash(dataset_id, model_id, str(query["id"]), results)
    if cache_key in cache:
        row = dict(cache[cache_key])
        row["cache_hit"] = True
        return row

    llm_model = str(judge_cfg.get("model", "openai/gpt-4.1-mini"))
    endpoint = str(
        judge_cfg.get("endpoint", "https://openrouter.ai/api/v1/chat/completions")
    )
    api_key_env = str(judge_cfg.get("api_key_env", "OPENROUTER_API_KEY"))
    temperature = float(judge_cfg.get("temperature", 0.0))

    api_key = os.getenv(api_key_env, "").strip()
    if not api_key:
        raise RuntimeError(f"missing env var {api_key_env}; required for judging")

    expected = query.get("expected_terms", [])
    if not isinstance(expected, list):
        expected = []
    expected_text = ", ".join(str(x) for x in expected if str(x).strip())
    context_k = max(1, int(judge_cfg.get("context_results", 5)))
    context_parts = []
    for r in results[:context_k]:
        context_parts.append(
            f"Rank {r['rank']}: {r['title']} ({r['url']})\nSnippet: {r['snippet']}"
        )
    context_blob = "\n\n".join(context_parts) if context_parts else "(no results)"

    system = (
        "You are a strict retrieval benchmark judge. Score relevance and grounding only from provided results."
    )
    user = f"""
Query: {query['query']}
Intent: {query.get('intent', '')}
Expected Terms: {expected_text}

Top Results:
{context_blob}

Return strict JSON:
{{
  "relevance_score": 0,
  "grounding_score": 0,
  "verdict": "poor|fair|good|excellent",
  "notes": "short reason"
}}

Scoring rubric (0-5):
- relevance_score: how well top results answer intent/query.
- grounding_score: how directly snippets support likely answer.
"""
    raw = call_openrouter(
        endpoint=endpoint,
        api_key=api_key,
        model=llm_model,
        messages=[{"role": "system", "content": system}, {"role": "user", "content": user}],
        temperature=temperature,
        max_tokens=350,
    )
    parsed = extract_json_object(raw)
    row = {
        "cache_key": cache_key,
        "llm_model": llm_model,
        "relevance_score": float(parsed.get("relevance_score", 0.0)),
        "grounding_score": float(parsed.get("grounding_score", 0.0)),
        "verdict": str(parsed.get("verdict", "unknown")),
        "notes": str(parsed.get("notes", "")),
        "cache_hit": False,
    }
    cache[cache_key] = row
    append_judge_cache(cache_path, row)
    return row


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    ensure_dirs(path.parent)
    if not rows:
        path.write_text("", encoding="utf-8")
        return
    fieldnames = sorted({k for row in rows for k in row.keys()})
    with path.open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        for row in rows:
            writer.writerow(row)


def maybe_write_parquet(csv_path: Path) -> str:
    try:
        import pyarrow.csv as pacsv  # type: ignore
        import pyarrow.parquet as pq  # type: ignore
    except Exception:
        return "pyarrow not available; parquet skipped"

    if not csv_path.exists() or csv_path.stat().st_size == 0:
        return "csv empty; parquet skipped"
    table = pacsv.read_csv(str(csv_path))
    parquet_path = csv_path.with_suffix(".parquet")
    pq.write_table(table, str(parquet_path))
    return f"wrote {parquet_path}"


def p95(values: list[float]) -> float:
    if not values:
        return 0.0
    sorted_vals = sorted(values)
    idx = int(round(0.95 * (len(sorted_vals) - 1)))
    return sorted_vals[idx]


def render_markdown_report(
    *,
    run_dir: Path,
    run_manifest: dict[str, Any] | None = None,
    output_path: Path | None = None,
) -> Path:
    index_csv = run_dir / "index_runs.csv"
    search_csv = run_dir / "search_results.csv"
    judge_csv = run_dir / "judgments.csv"
    if not index_csv.exists() or not search_csv.exists():
        raise FileNotFoundError(f"missing benchmark CSV files in {run_dir}")

    index_rows = list(csv.DictReader(index_csv.open("r", encoding="utf-8")))
    search_rows = list(csv.DictReader(search_csv.open("r", encoding="utf-8")))
    judge_rows = (
        list(csv.DictReader(judge_csv.open("r", encoding="utf-8")))
        if judge_csv.exists() and judge_csv.stat().st_size > 0
        else []
    )
    hit_cols = sorted(
        {k for row in search_rows for k in row.keys() if k.startswith("hit_at_")},
        key=lambda x: int(x.split("_")[-1]),
    )
    ndcg_cols = sorted(
        {k for row in search_rows for k in row.keys() if k.startswith("ndcg_at_")},
        key=lambda x: int(x.split("_")[-1]),
    )

    dataset_rows: dict[str, dict[str, Any]] = {}
    for row in index_rows:
        ds = row["dataset_id"]
        if ds not in dataset_rows:
            dataset_rows[ds] = {
                "dataset_id": ds,
                "size_tier": row.get("size_tier", ""),
                "file_count": int(float(row.get("dataset_file_count", "0") or "0")),
                "doc_count": int(float(row.get("dataset_doc_count", "0") or "0")),
                "total_mb": (int(float(row.get("dataset_total_bytes", "0") or "0")) / (1024 * 1024)),
            }

    grouped: dict[tuple[str, str], dict[str, Any]] = {}
    for row in index_rows:
        key = (row["dataset_id"], row["model_id"])
        g = grouped.setdefault(
            key,
            {
                "dataset_id": row["dataset_id"],
                "model_id": row["model_id"],
                "size_tier": row.get("size_tier", ""),
                "index_secs": [],
                "index_sizes_mb": [],
                "model_cache_mb": [],
                "model_weights_mb": [],
                "search_ms": [],
                "judge_rel": [],
                "judge_ground": [],
                "relevance_count": 0,
                "relevance_weights": [],
                "mrr": [],
                **{c: [] for c in hit_cols},
                **{c: [] for c in ndcg_cols},
            },
        )
        g["index_secs"].append(float(row.get("index_duration_s", "0") or "0"))
        g["index_sizes_mb"].append(float(row.get("index_size_bytes", "0") or "0") / (1024 * 1024))
        cache_mb = float(row.get("model_cache_bytes", "0") or "0") / (1024 * 1024)
        weights_mb = float(row.get("model_weights_bytes", "0") or "0") / (1024 * 1024)
        if cache_mb <= 0.0 and weights_mb <= 0.0:
            inferred = model_cache_stats(REPO_ROOT / ".bench" / "cache", row["model_id"])
            cache_mb = float(inferred.get("model_cache_bytes", 0)) / (1024 * 1024)
            weights_mb = float(inferred.get("model_weights_bytes", 0)) / (1024 * 1024)
        g["model_cache_mb"].append(cache_mb)
        g["model_weights_mb"].append(weights_mb)

    for row in search_rows:
        key = (row["dataset_id"], row["model_id"])
        if key not in grouped:
            continue
        grouped[key]["search_ms"].append(float(row.get("latency_ms", "0") or "0"))
        is_labeled = float(row.get("relevance_labeled", "0") or "0") > 0.0
        if is_labeled:
            grouped[key]["relevance_count"] += 1
            grouped[key]["relevance_weights"].append(
                float(row.get("relevance_label_weight", "1") or "1")
            )
            grouped[key]["mrr"].append(float(row.get("mrr", "0") or "0"))
            for c in hit_cols:
                grouped[key][c].append(float(row.get(c, "0") or "0"))
            for c in ndcg_cols:
                grouped[key][c].append(float(row.get(c, "0") or "0"))

    for row in judge_rows:
        key = (row["dataset_id"], row["model_id"])
        if key not in grouped:
            continue
        grouped[key]["judge_rel"].append(float(row.get("relevance_score", "0") or "0"))
        grouped[key]["judge_ground"].append(float(row.get("grounding_score", "0") or "0"))

    lines: list[str] = []
    lines.append("# Eddie Benchmark Report")
    lines.append("")
    lines.append(f"- Generated at: {utc_now()}")
    lines.append(f"- Run directory: `{run_dir}`")
    if run_manifest:
        lines.append(f"- Suite: `{run_manifest.get('suite_name', '')}`")
    lines.append("")

    lines.append("## Dataset Profile")
    lines.append("")
    lines.append("| Dataset | Tier | Files | Docs | Text Size (MB) |")
    lines.append("|---|---|---:|---:|---:|")
    for row in sorted(
        dataset_rows.values(),
        key=lambda r: (TIER_ORDER.get(str(r["size_tier"]).lower(), 99), r["dataset_id"]),
    ):
        lines.append(
            f"| {row['dataset_id']} | {row['size_tier']} | {row['file_count']} | {row['doc_count']} | {row['total_mb']:.1f} |"
        )

    lines.append("")
    lines.append("## Performance Matrix")
    lines.append("")
    headers = [
        "Dataset",
        "Tier",
        "Model",
        "Model Cache Mean (MB)",
        "Weights File (MB)",
        "Index Time Mean (s)",
        "Index Size Mean (MB)",
        "Query Median (ms)",
        "Query P95 (ms)",
    ]
    if hit_cols or ndcg_cols:
        headers.append("Labeled Q")
        headers.extend([f"Hit@{c.split('_')[-1]}" for c in hit_cols])
        headers.append("MRR")
        headers.extend([f"nDCG@{c.split('_')[-1]}" for c in ndcg_cols])
    headers.extend(["LLM Relevance Mean (0-5)", "LLM Grounding Mean (0-5)"])
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join(["---"] * 3 + ["---:"] * (len(headers) - 3)) + "|")

    rows_sorted = sorted(
        grouped.values(),
        key=lambda r: (
            TIER_ORDER.get(str(r["size_tier"]).lower(), 99),
            r["dataset_id"],
            r["model_id"],
        ),
    )
    for row in rows_sorted:
        rel = statistics.mean(row["judge_rel"]) if row["judge_rel"] else 0.0
        grd = statistics.mean(row["judge_ground"]) if row["judge_ground"] else 0.0
        parts = [
            row["dataset_id"],
            row["size_tier"],
            row["model_id"],
            f"{statistics.mean(row['model_cache_mb']) if row['model_cache_mb'] else 0.0:.2f}",
            f"{statistics.mean(row['model_weights_mb']) if row['model_weights_mb'] else 0.0:.2f}",
            f"{statistics.mean(row['index_secs']) if row['index_secs'] else 0.0:.2f}",
            f"{statistics.mean(row['index_sizes_mb']) if row['index_sizes_mb'] else 0.0:.2f}",
            f"{statistics.median(row['search_ms']) if row['search_ms'] else 0.0:.1f}",
            f"{p95(row['search_ms']):.1f}",
        ]
        if hit_cols or ndcg_cols:
            parts.append(str(int(row["relevance_count"])))
            for c in hit_cols:
                parts.append(
                    f"{statistics.mean(row[c]) if row[c] else 0.0:.2f}"
                )
            parts.append(f"{statistics.mean(row['mrr']) if row['mrr'] else 0.0:.2f}")
            for c in ndcg_cols:
                parts.append(
                    f"{statistics.mean(row[c]) if row[c] else 0.0:.2f}"
                )
        parts.extend([f"{rel:.2f}", f"{grd:.2f}"])
        lines.append("| " + " | ".join(parts) + " |")

    lines.append("")
    lines.append("## Notes")
    lines.append("")
    lines.append("- Query latency here is CLI end-to-end (`eddie search`), including model load overhead.")
    if hit_cols or ndcg_cols:
        lines.append("- Deterministic retrieval metrics use `benchmarks/relevance_labels.toml` query labels.")
    lines.append("- LLM judging is heuristic and should be compared relatively across runs.")
    lines.append("- For browser runtime latency, add a dedicated wasm/web benchmark lane.")
    lines.append("")

    out = output_path if output_path else (run_dir / "benchmark_report.md")
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return out


def cmd_prepare(args: argparse.Namespace) -> int:
    suite, _, _, _, qgen_cfg, _, datasets, _ = load_config(Path(args.config))
    ensure_dirs(suite.cache_root, suite.output_root)
    for ds in filter_datasets(datasets, args.dataset):
        resolved = ensure_dataset_cached(ds, suite.cache_root, refresh=args.refresh)
        info(
            f"[dataset:{ds.dataset_id}] ready at {resolved.content_dir} "
            f"({resolved.file_count} files, {resolved.total_bytes} bytes)"
        )
        if args.generate_queries:
            generate_queries_for_dataset(ds=resolved, qgen_cfg=qgen_cfg, force=args.force_queries)
    return 0


def filter_datasets(datasets: list[DatasetConfig], dataset_ids: list[str]) -> list[DatasetConfig]:
    if not dataset_ids:
        return datasets
    want = set(dataset_ids)
    out = [d for d in datasets if d.dataset_id in want]
    if len(out) != len(want):
        missing = sorted(want - {d.dataset_id for d in out})
        raise ValueError(f"unknown dataset id(s): {', '.join(missing)}")
    return out


def filter_models(models: list[ModelConfig], model_ids: list[str]) -> list[ModelConfig]:
    if not model_ids:
        return models
    want = set(model_ids)
    out = [m for m in models if m.model_id in want]
    if len(out) != len(want):
        missing = sorted(want - {m.model_id for m in out})
        raise ValueError(f"unknown model id(s): {', '.join(missing)}")
    return out


def cmd_generate_queries(args: argparse.Namespace) -> int:
    suite, _, _, _, qgen_cfg, _, datasets, _ = load_config(Path(args.config))
    ensure_dirs(suite.cache_root, suite.output_root)
    for ds in filter_datasets(datasets, args.dataset):
        resolved = ensure_dataset_cached(ds, suite.cache_root, refresh=args.refresh_datasets)
        generate_queries_for_dataset(ds=resolved, qgen_cfg=qgen_cfg, force=args.force)
    return 0


def cmd_run(args: argparse.Namespace) -> int:
    suite, index_cfg, search_cfg, judge_cfg, qgen_cfg, relevance_cfg, datasets, models = load_config(
        Path(args.config)
    )
    selected_datasets = filter_datasets(datasets, args.dataset)
    selected_models = filter_models(models, args.model)

    ensure_dirs(suite.cache_root, suite.output_root)
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = suite.output_root / run_id
    ensure_dirs(run_dir)

    bench_env = benchmark_env(suite.cache_root)
    if args.hf_home:
        bench_env["HF_HOME"] = str(resolve_repo_path(args.hf_home))
        bench_env["HUGGINGFACE_HUB_CACHE"] = str(resolve_repo_path(args.hf_home) / "hub")
        bench_env["TRANSFORMERS_CACHE"] = str(resolve_repo_path(args.hf_home) / "transformers")

    runs = int(args.runs_per_combo or suite.runs_per_combo)
    judge_enabled = bool(judge_cfg.get("enabled", True)) and not args.no_judge
    judge_limit = int(judge_cfg.get("max_queries_per_combo", 12))
    relevance_enabled = bool(relevance_cfg.get("enabled", True))
    relevance_k_values = parse_k_values(relevance_cfg.get("k_values", [1, 3, 5]), [1, 3, 5])
    relevance_ndcg_k = int(relevance_cfg.get("ndcg_k", 5))
    if relevance_ndcg_k <= 0:
        relevance_ndcg_k = 5
    labels_file = resolve_repo_path(
        str(relevance_cfg.get("labels_file", "benchmarks/relevance_labels.toml"))
    )
    labels = load_relevance_labels(labels_file) if relevance_enabled else {}

    manifest = {
        "run_id": run_id,
        "suite_name": suite.name,
        "generated_at_utc": utc_now(),
        "config_path": str(Path(args.config).resolve()),
        "eddie_bin": str(suite.eddie_bin),
        "runs_per_combo": runs,
        "judge_enabled": judge_enabled,
        "relevance_enabled": relevance_enabled,
        "relevance_k_values": relevance_k_values,
        "relevance_ndcg_k": relevance_ndcg_k,
        "relevance_labels_file": str(labels_file),
        "relevance_label_count": len(labels),
        "datasets": [d.dataset_id for d in selected_datasets],
        "models": [m.model_id for m in selected_models],
        "hf_home": bench_env.get("HF_HOME", ""),
    }
    (run_dir / "run_manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8"
    )

    index_rows: list[dict[str, Any]] = []
    search_rows: list[dict[str, Any]] = []
    judge_rows: list[dict[str, Any]] = []
    judge_counts: dict[tuple[str, str], int] = {}

    judge_cache_path = suite.cache_root / "judging_cache.jsonl"
    judge_cache = load_judge_cache(judge_cache_path)

    for ds in selected_datasets:
        resolved = ensure_dataset_cached(ds, suite.cache_root, refresh=args.refresh_datasets)
        if args.generate_queries and (args.force_queries or not ds.query_file.exists()):
            generate_queries_for_dataset(
                ds=resolved, qgen_cfg=qgen_cfg, force=args.force_queries
            )
        queries = load_queries(ds.query_file)
        if args.query_limit:
            queries = queries[: int(args.query_limit)]
        if not queries:
            raise RuntimeError(f"no queries for dataset {ds.dataset_id}")

        for model in selected_models:
            for repeat in range(1, runs + 1):
                combo_slug = f"{ds.dataset_id}__{slugify_model(model.model_id)}__r{repeat}"
                combo_dir = run_dir / "artifacts" / combo_slug
                ensure_dirs(combo_dir)
                index_path = combo_dir / "index.ed"

                idx_cmd = [
                    str(suite.eddie_bin),
                    "index",
                    "--content-dir",
                    str(resolved.content_dir),
                    "--output",
                    str(index_path),
                    "--model",
                    model.model_id,
                    "--chunk-size",
                    str(index_cfg.get("chunk_size", 256)),
                    "--overlap",
                    str(index_cfg.get("overlap", 32)),
                    "--chunk-strategy",
                    str(index_cfg.get("chunk_strategy", "heading")),
                ]
                if bool(index_cfg.get("summary_lane", False)):
                    idx_cmd.append("--summary-lane")
                if bool(index_cfg.get("qa", False)):
                    idx_cmd.append("--qa")
                if bool(index_cfg.get("claims", False)):
                    idx_cmd.append("--claims")
                if index_cfg.get("coarse_chunk_size") is not None:
                    idx_cmd.extend(
                        ["--coarse-chunk-size", str(index_cfg.get("coarse_chunk_size"))]
                    )
                if index_cfg.get("coarse_overlap") is not None:
                    idx_cmd.extend(["--coarse-overlap", str(index_cfg.get("coarse_overlap"))])

                info(
                    f"[run:{run_id}] dataset={ds.dataset_id} model={model.model_id} repeat={repeat} indexing..."
                )
                t0 = time.perf_counter()
                proc = run_cmd(idx_cmd, env=bench_env, capture=True, check=True)
                index_secs = time.perf_counter() - t0
                idx_stdout = proc.stdout or ""
                idx_stderr = proc.stderr or ""
                chunks, qa_entries, claims_entries = parse_index_summary(idx_stderr)
                index_bytes = index_path.stat().st_size if index_path.exists() else 0
                model_sizes = model_cache_stats(suite.cache_root, model.model_id)
                index_rows.append(
                    {
                        "run_id": run_id,
                        "dataset_id": ds.dataset_id,
                        "size_tier": ds.size_tier,
                        "model_id": model.model_id,
                        "repeat": repeat,
                        "index_duration_s": round(index_secs, 4),
                        "index_size_bytes": index_bytes,
                        "chunk_count": chunks or 0,
                        "qa_entries": qa_entries or 0,
                        "claims_entries": claims_entries or 0,
                        "dataset_file_count": resolved.file_count,
                        "dataset_doc_count": resolved.doc_count,
                        "dataset_total_bytes": resolved.total_bytes,
                        "dataset_revision": resolved.git_revision,
                        "model_cache_bytes": model_sizes["model_cache_bytes"],
                        "model_weights_bytes": model_sizes["model_weights_bytes"],
                        "model_tokenizer_bytes": model_sizes["model_tokenizer_bytes"],
                        "model_config_bytes": model_sizes["model_config_bytes"],
                        "index_cmd": " ".join(idx_cmd),
                        "index_stdout_hash": hashlib.sha256(idx_stdout.encode("utf-8")).hexdigest(),
                        "index_stderr_hash": hashlib.sha256(idx_stderr.encode("utf-8")).hexdigest(),
                    }
                )

                mode = str(search_cfg.get("mode", "hybrid"))
                scope = str(search_cfg.get("scope", "chunks"))
                top_k = int(search_cfg.get("top_k", 5))

                for q in queries:
                    qid = str(q["id"])
                    qtext = str(q["query"])
                    search_cmd = [
                        str(suite.eddie_bin),
                        "search",
                        "--index",
                        str(index_path),
                        "--query",
                        qtext,
                        "--top-k",
                        str(top_k),
                        "--model",
                        model.model_id,
                        "--mode",
                        mode,
                        "--scope",
                        scope,
                    ]
                    t1 = time.perf_counter()
                    sproc = run_cmd(search_cmd, env=bench_env, capture=True, check=True)
                    latency_ms = (time.perf_counter() - t1) * 1000.0
                    results = parse_search_output(sproc.stdout or "")
                    top = results[0] if results else {}
                    label = labels.get((ds.dataset_id, qid)) if relevance_enabled else None
                    rel_metrics = score_relevance_metrics(
                        results=results,
                        label=label,
                        k_values=relevance_k_values,
                        ndcg_k=relevance_ndcg_k,
                    )
                    search_rows.append(
                        {
                            "run_id": run_id,
                            "dataset_id": ds.dataset_id,
                            "size_tier": ds.size_tier,
                            "model_id": model.model_id,
                            "repeat": repeat,
                            "query_id": qid,
                            "query": qtext,
                            "intent": q.get("intent", ""),
                            "difficulty": q.get("difficulty", ""),
                            "latency_ms": round(latency_ms, 3),
                            "result_count": len(results),
                            "top1_title": top.get("title", ""),
                            "top1_url": top.get("url", ""),
                            "top1_snippet": top.get("snippet", ""),
                            "results_json": json.dumps(results[:top_k], ensure_ascii=False),
                            **rel_metrics,
                        }
                    )

                    if judge_enabled and repeat == 1:
                        combo_key = (ds.dataset_id, model.model_id)
                        already = judge_counts.get(combo_key, 0)
                        if judge_limit <= 0 or already < judge_limit:
                            judged = judge_results(
                                judge_cfg=judge_cfg,
                                dataset_id=ds.dataset_id,
                                model_id=model.model_id,
                                query=q,
                                results=results,
                                cache=judge_cache,
                                cache_path=judge_cache_path,
                            )
                            judge_rows.append(
                                {
                                    "run_id": run_id,
                                    "dataset_id": ds.dataset_id,
                                    "size_tier": ds.size_tier,
                                    "model_id": model.model_id,
                                    "repeat": repeat,
                                    "query_id": qid,
                                    "query": qtext,
                                    "relevance_score": judged["relevance_score"],
                                    "grounding_score": judged["grounding_score"],
                                    "verdict": judged["verdict"],
                                    "notes": judged["notes"],
                                    "llm_model": judged["llm_model"],
                                    "cache_hit": judged.get("cache_hit", False),
                                }
                            )
                            judge_counts[combo_key] = already + 1

                if not suite.keep_indexes:
                    try:
                        index_path.unlink(missing_ok=True)
                    except OSError:
                        pass

    index_csv = run_dir / "index_runs.csv"
    search_csv = run_dir / "search_results.csv"
    judge_csv = run_dir / "judgments.csv"
    write_csv(index_csv, index_rows)
    write_csv(search_csv, search_rows)
    write_csv(judge_csv, judge_rows)

    info(f"[run:{run_id}] wrote {index_csv}")
    info(f"[run:{run_id}] wrote {search_csv}")
    info(f"[run:{run_id}] wrote {judge_csv}")
    info(f"[run:{run_id}] {maybe_write_parquet(index_csv)}")
    info(f"[run:{run_id}] {maybe_write_parquet(search_csv)}")
    info(f"[run:{run_id}] {maybe_write_parquet(judge_csv)}")

    report_path = render_markdown_report(run_dir=run_dir, run_manifest=manifest)
    info(f"[run:{run_id}] wrote {report_path}")
    print(str(run_dir))
    return 0


def cmd_render(args: argparse.Namespace) -> int:
    run_dir = Path(args.run_dir).resolve()
    manifest_path = run_dir / "run_manifest.json"
    manifest = None
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    out = Path(args.output).resolve() if args.output else None
    report = render_markdown_report(run_dir=run_dir, run_manifest=manifest, output_path=out)
    print(report)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Eddie benchmarking suite (datasets/models matrix + LLM judging)"
    )
    parser.add_argument("--config", default=str(DEFAULT_CONFIG), help="Path to benchmark TOML config")
    sub = parser.add_subparsers(dest="command", required=True)

    p_prepare = sub.add_parser("prepare", help="Download/cache datasets (optionally generate query files)")
    p_prepare.add_argument("--dataset", action="append", default=[], help="Dataset ID filter (repeatable)")
    p_prepare.add_argument("--refresh", action="store_true", help="Refresh cached datasets")
    p_prepare.add_argument(
        "--generate-queries", action="store_true", help="Generate queries for selected datasets"
    )
    p_prepare.add_argument("--force-queries", action="store_true", help="Overwrite existing query files")
    p_prepare.set_defaults(func=cmd_prepare)

    p_q = sub.add_parser("generate-queries", help="Generate benchmark queries using OpenRouter")
    p_q.add_argument("--dataset", action="append", default=[], help="Dataset ID filter (repeatable)")
    p_q.add_argument("--refresh-datasets", action="store_true", help="Refresh cached datasets first")
    p_q.add_argument("--force", action="store_true", help="Overwrite existing query files")
    p_q.set_defaults(func=cmd_generate_queries)

    p_run = sub.add_parser("run", help="Run full benchmark matrix")
    p_run.add_argument("--dataset", action="append", default=[], help="Dataset ID filter (repeatable)")
    p_run.add_argument("--model", action="append", default=[], help="Model ID filter (repeatable)")
    p_run.add_argument(
        "--runs-per-combo", type=int, default=0, help="Override repeats per dataset/model combo"
    )
    p_run.add_argument("--query-limit", type=int, default=0, help="Limit queries per dataset")
    p_run.add_argument("--refresh-datasets", action="store_true", help="Refresh cached datasets")
    p_run.add_argument(
        "--generate-queries",
        action="store_true",
        help="Generate missing query files before running",
    )
    p_run.add_argument("--force-queries", action="store_true", help="Force query file regeneration")
    p_run.add_argument("--no-judge", action="store_true", help="Disable OpenRouter judging")
    p_run.add_argument(
        "--hf-home",
        default="",
        help="Override HF_HOME cache location (relative to repo or absolute)",
    )
    p_run.set_defaults(func=cmd_run)

    p_render = sub.add_parser("render-report", help="Render markdown summary table from a run directory")
    p_render.add_argument("run_dir", help="Path to benchmark run directory")
    p_render.add_argument("--output", default="", help="Optional markdown output path")
    p_render.set_defaults(func=cmd_render)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    if hasattr(args, "runs_per_combo") and args.runs_per_combo == 0:
        args.runs_per_combo = None
    if hasattr(args, "query_limit") and args.query_limit == 0:
        args.query_limit = None
    if hasattr(args, "output") and args.output == "":
        args.output = None

    try:
        return int(args.func(args))
    except Exception as exc:
        err(str(exc))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
