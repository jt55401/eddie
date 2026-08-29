# Retrieval tuning, 2026-08-29

Starting point: on www.jason-grey.com the query "how long has jason been programming?" put the answering page (`/skills/programming-languages/`, "coding since age 6 or 7… nearly 40 years") second behind a blog post, and the FAQ card showed an unrelated "years of experience in AI/ML" entry.

## Labelled sets

| set | pages | chunks | questions | source |
|---|---:|---:|---:|---|
| jason-grey.com (résumé site) | 75 | 361–424 | 45 graded | written from the content; 15 factual, 10 skills, 10 opinions, 5 projects, 5 casual/typo |
| anthrosim.com (business site) | 22 | 482 (HTML) | 35 graded | product, pricing, research papers, use cases, casual |
| EnterpriseRAG-Bench, Confluence subset | 5,189 | 76,082 | 64 (8 question types) | `/bigdata/semantic-eval/rag/enterpriserag-bench`, questions whose expected docs are all Confluence pages |

Metrics are graded nDCG@10, MRR and Hit@10 from `eddie eval --graded`. All numbers below are hybrid mode with the bge-small lane unless stated.

## What moved the numbers

| change | jason MRR / nDCG | anthrosim MRR / nDCG | erag MRR / nDCG |
|---|---|---|---|
| baseline (v0.4.0 index: no title context, no summary chunk, weights 1/1/0.8) | 0.705 / 0.671 | 0.676 / 0.550 | 0.621 / 0.624 (dash-normalised URLs) |
| + summary chunk (title, description, headings) | 0.764 / 0.705 | | |
| + title/section prefix in the indexed text | 0.807 / 0.768 | 0.674 / 0.593 (markdown) | |
| + `--cms html --include-noindex` (AnthroSim keeps its copy in templates) | | 0.798 / 0.748, Hit@10 1.000 | |
| + weights 1.2/0.8/0.6 (best on the two small sites) | 0.822 / 0.775 | 0.708 / 0.607 | 0.621 / 0.624 (worst row on ERAG) |
| + weights 1/1.2/1 (best mean and worst case across the three) | 0.765–0.822 | ≈0.70 / 0.60 | 0.732 / 0.706 |
| Qwen3-Embedding lane instead of bge-small | 0.838 / 0.776 | | 0.761 / 0.730 |

Per-arm on ERAG (Qwen3 lane): dense alone 0.605 / 0.598, sparse alone 0.702 / 0.679, keyword alone 0.729 / 0.722, hybrid 0.761 / 0.730. By question type (hybrid, Qwen3 lane): basic 0.83 MRR, constrained 1.00, completeness 0.82, semantic 0.38 (Hit@10 0.53). The "semantic" class (roundabout phrasing, little keyword overlap) is where small embedders fall short.

## Decisions

- Title and section are prefixed to the text that BM25, the sparse encoder and the dense lane see; the stored text stays clean. `--no-title-context` turns it off; the manifest records `title_context`.
- The per-page summary chunk is on by default (`--no-summary-lane` to skip).
- Default fusion weights are dense 1.0, sparse 1.2, BM25 1.0. The small-site optimum (1.2/0.8/0.6) is the worst setting on enterprise docs, so the default is the most robust row, and `eddie index --weights D,S,B` bakes a site's own sweep result into the manifest; the widget honours it.
- `--cms html` indexes built output; `--include-noindex` keeps pages the site marks noindex.
- Hugo URLs keep `_ . ~ + @` like Hugo's URLize (previously `_` became `-`, which is why every ERAG label missed).
- QA lane: `--qa-subject "Jason Grey"` names the owner in synthesized questions (and rewrites "the author"/"the subject"/"the site owner"); `rank_qa` fuses dense score, IDF-weighted term overlap and a BM25 pass over question+answer; only the top hit can be `confident`; the widget shows the FAQ card only when it is, and hands confident entries to the agent as evidence. Ollama thinking is disabled for synthesis (`think: false`).

## Result on the motivating query

`eddie qa --query "how long has jason been programming?"`: #1 confident "How long has Jason Grey been coding? → since age 6 or 7, nearly 40 years" (`/skills/programming-languages/`). In the browser with the Qwen3 lane the page is the #1 result and the agent answers "Jason has been coding since age 6 or 7, which is nearly 40 years. [3]".

Label files: `~/tmp/eddie-eval/{jasongrey,anthrosim}.labels.toml`, `~/tmp/eddie-eval/erag/erag.labels.toml` (the résumé-site labels are also committed to the site repo under `.eddie/`).
