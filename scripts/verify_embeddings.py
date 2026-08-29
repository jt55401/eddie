#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Reference vectors for Eddie's embedding cross-check.

Writes, into --out (default ~/tmp/eddie-ref):
  <lane>.json   {"model": id, "docs": [[f32]], "queries": [[f32]]}  for each dense model
  sparse.json   {"model": id, "texts": [...], "top10": [[[token_id, weight], ...], ...]}

Then run the Rust side, which loads the same models through candle and
compares (cosine >= 0.999 for BERT/XLM-R, >= 0.99 for Qwen3; sparse top-10
term ids must match):

  EDDIE_REF_DIR=~/tmp/eddie-ref cargo test --release -- --ignored compare_with

Setup (CPU torch is enough):
  uv venv ~/tmp/st-venv
  uv pip install --python ~/tmp/st-venv/bin/python sentence-transformers torch transformers
  ~/tmp/st-venv/bin/python scripts/verify_embeddings.py
"""

import argparse
import json
import os
import sys

# Keep in sync with the ignored tests in src/embed.rs and src/sparse.rs.
TEXTS = [
    "The quick brown fox jumps over the lazy dog.",
    "How do I configure the search widget on a Hugo site?",
    "Eddie builds a semantic index at build time and searches it in the browser.",
    "Photosynthesis converts light energy into chemical energy in plants.",
    "The 2024 release added CUDA support for indexing.",
]
SPARSE_TEXTS = [
    "Currently New York is rainy.",
    "Eddie builds a semantic index at build time and searches it in the browser.",
    "Photosynthesis converts light energy into chemical energy in plants.",
]
DENSE_MODELS = {
    "minilm": ("sentence-transformers/multi-qa-MiniLM-L6-cos-v1", ""),
    "bge-small": (
        "BAAI/bge-small-en-v1.5",
        "Represent this sentence for searching relevant passages: ",
    ),
    "bge-m3": ("BAAI/bge-m3", ""),
    "qwen3e": (
        "Qwen/Qwen3-Embedding-0.6B",
        "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ",
    ),
}
SPARSE_MODEL = "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill"


def dense(out_dir, lanes):
    from sentence_transformers import SentenceTransformer

    for lane, (model_id, query_prefix) in DENSE_MODELS.items():
        if lanes and lane not in lanes:
            continue
        print(f"[dense] {model_id}", file=sys.stderr)
        model = SentenceTransformer(model_id, device="cpu")
        docs = model.encode(TEXTS, normalize_embeddings=True, batch_size=8)
        queries = model.encode(
            TEXTS, prompt=query_prefix, normalize_embeddings=True, batch_size=8
        )
        with open(os.path.join(out_dir, f"{lane}.json"), "w") as f:
            json.dump(
                {
                    "model": model_id,
                    "max_seq_length": model.max_seq_length,
                    "docs": docs.tolist(),
                    "queries": queries.tolist(),
                },
                f,
            )


def sparse(out_dir):
    import torch
    from transformers import AutoModelForMaskedLM, AutoTokenizer

    print(f"[sparse] {SPARSE_MODEL}", file=sys.stderr)
    model = AutoModelForMaskedLM.from_pretrained(SPARSE_MODEL)
    tokenizer = AutoTokenizer.from_pretrained(SPARSE_MODEL)
    special_token_ids = [
        tokenizer.vocab[token] for token in tokenizer.special_tokens_map.values()
    ]
    feature = tokenizer(
        SPARSE_TEXTS,
        padding=True,
        truncation=True,
        return_tensors="pt",
        return_token_type_ids=False,
    )
    with torch.no_grad():
        output = model(**feature)[0]
    # Model card, v3: log(1 + log(1 + relu(max over positions)))
    values, _ = torch.max(output * feature["attention_mask"].unsqueeze(-1), dim=1)
    values = torch.log(1 + torch.log(1 + torch.relu(values)))
    values[:, special_token_ids] = 0
    top10 = []
    for row in values:
        w, idx = torch.topk(row, 10)
        top10.append([[int(i), float(x)] for i, x in zip(idx.tolist(), w.tolist())])
    with open(os.path.join(out_dir, "sparse.json"), "w") as f:
        json.dump({"model": SPARSE_MODEL, "texts": SPARSE_TEXTS, "top10": top10}, f)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.expanduser("~/tmp/eddie-ref"))
    ap.add_argument("--lanes", nargs="*", help="dense lanes to run (default all)")
    ap.add_argument("--skip-sparse", action="store_true")
    ap.add_argument("--skip-dense", action="store_true")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    if not args.skip_dense:
        dense(args.out, args.lanes)
    if not args.skip_sparse:
        sparse(args.out)
    print(f"wrote references to {args.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
