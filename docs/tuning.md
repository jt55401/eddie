<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Tuning

How to measure whether Eddie returns the right pages for your site, and how
to improve it when it does not.

Start here if search "feels wrong". Guessing at settings without measuring
usually makes results worse, and you will not know which change did it.

## Contents

- [Write a label set](#write-a-label-set)
- [Score the index](#score-the-index)
- [Adjust the ranking](#adjust-the-ranking)
- [Adjust chunk size](#adjust-chunk-size)
- [Answer card ranking](#answer-card-ranking)
- [Correcting extracted facts](#correcting-extracted-facts)

## Write a label set

A label set is a list of questions and the pages that should answer them.
Keep it in your site's repository, not in Eddie.

```toml
[[cases]]
id = "programming-years"
query = "how long has jason been programming?"
relevant = ["/skills/programming-languages/"]

[cases.graded]
"/skills/programming-languages/" = 3
"/r/" = 1
```

`relevant` lists pages that count as correct. The optional `[cases.graded]`
table rates them 1 to 3, where 3 answers the question directly and 1 only
mentions it. Graded scoring rewards putting the best page first, not merely
a relevant one.

Twenty to fifty questions is enough to see real differences. Write the ones
your visitors actually ask.

`examples/acceptance-suite.json` is a starting point you can copy.

## Score the index

```bash
eddie eval --index index.ed --labels labels.toml --graded
```

You get three numbers:

| Metric | What it means |
|---|---|
| Hit@10 | How often a correct page appears at all |
| MRR | How high the first correct page ranks |
| nDCG@10 | How well the whole ranking matches the grades |

Compare runs, not absolute values. A change is real when it moves MRR or
nDCG by more than about 0.01 across the whole set.

Useful variations:

```bash
eddie eval --index index.ed --labels labels.toml --all-modes   # each search method on its own
eddie eval --index index.ed --labels labels.toml --sweep       # 48 weightings, best first
```

`--all-modes` shows keywords, learned terms and meaning separately, which
tells you which one is carrying the results and which is letting you down.

Queries are embedded once per run, so `--sweep` and `--all-modes` are cheap.

## Adjust the ranking

Eddie merges the three result lists with weighted reciprocal rank fusion.
Each method's rank counts for a share of the final score, set by its weight.

The defaults are meaning 1.0, learned terms 1.2, keywords 1.0. They were
chosen for the best worst-case across three labelled sets: a personal site,
a business site, and part of EnterpriseRAG's Confluence corpus.

Find better weights for your site, then bake them in:

```bash
eddie eval  --index index.ed --labels labels.toml --sweep      # find the best row
eddie index --content-dir content/ --output index.ed --weights 1.2,0.8,0.6
```

The widget then uses those weights. `eddie search --weights` tries a setting
by hand without rebuilding.

Two more knobs, both rarely needed: `--fetch-k` is how many candidates each
method contributes before merging (default is 30 or three times your
`--top-k`, whichever is larger), and `--rrf-k` is the fusion constant
(default 60). Larger `--rrf-k` flattens the influence of rank position.

## Adjust chunk size

Eddie splits pages into chunks and searches those. Chunks that are too small
lose context; too large and the match gets diluted.

Sweep both parameters and let the labels decide:

```bash
eddie tune \
  --content-dir content/ \
  --eval labels.toml \
  --chunk-sizes 192,256,320 \
  --overlaps 16,32,48 \
  --mode hybrid \
  --report tune-report.json
```

Or collect labels as you go, by rating results interactively:

```bash
eddie tune --content-dir content/ --interactive --save-eval labels.toml
```

## Answer card ranking

When you index with `--qa`, Eddie writes a set of questions and answers into
the index. The widget shows the best match above the results, and `eddie qa`
ranks them the same way:

```text
lexical = 0.5 × overlap + 0.5 × bm25_norm
score   = 0.6 × meaning + 0.4 × lexical
```

`overlap` is the share of the query's content words found in the entry, with
stop words dropped. `bm25_norm` is the entry's keyword score relative to the
best entry. An entry is shown as confident when `score` is at least 0.55 and
either `overlap` is at least 0.34 or the meaning score is at least 0.80.

Question synthesis writes "the author" unless you tell it the subject:

```bash
eddie index --content-dir content/ --output index.ed --qa --qa-subject "Jason Grey"
```

Use the name your visitors would type.

## Correcting extracted facts

`--claims` extracts facts from your pages. Extraction gets things wrong.
Correct it with a `claims.edits.toml` rather than editing the index:

```toml
[[redact]]
predicate = "worked_for"
object = "Old Company"

[[add]]
subject = "Site Subject"
predicate = "worked_for"
object = "Nike"
evidence = "Manual correction"
source_url = "/about/"
confidence = 1.0
tags = ["manual"]
```

Apply it when you index:

```bash
eddie index --content-dir content/ --output index.ed --claims --claims-edits claims.edits.toml
```

`examples/claims.edits.toml` is a template.

## Related documents

- [Reference](reference.md) — every flag these commands accept
- [Benchmarks](benchmarks.md) — how Eddie is measured across datasets
- [Retrieval tuning review](reviews/2026-08-29-retrieval-tuning.md) — a worked example on a real site
