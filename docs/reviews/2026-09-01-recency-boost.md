<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Recency in the final ranking: what it does to answer quality

Date: 2026-09-01. Asked for as "an optional/on by default boost in the final
ranking for newer pages", with the explicit condition "make sure our answers
are still accurate". This is the accuracy half.

## What was built

`manifest.recency` (a [`RecencySpec`]) carries a `strength` and a
`half_life_days`, baked in by `eddie index --recency` / `--recency-half-life`
and left out entirely by `--no-recency`. On a browse-style query (see "The
gate" below) `group_pages` multiplies each page's fused score by

```
1 + strength * 0.5 ^ (age_days / half_life_days)
```

so a page dated as recently as the newest page in the corpus gets the full
`strength` and one half-life older gets half of it. A page with no date, or
an unparseable one, is not moved either way -- the boost only ever lifts,
never demotes.

Ages are measured from the newest date in the corpus, not from the clock, so
an index ranks the same way whenever it is searched. `eddie search` and
`eddie eval` take `--recency` / `--recency-half-life` to override what the
index carries, which is how the numbers below were produced.

## First attempt: ungated, and it made answers worse

Applied to every query regardless of kind. 45 labelled questions, graded
nDCG, the site's own index and weights (1.2/0.8/0.6), half-life 240 days:

| strength | Hit@10 | MRR | nDCG@10 |
|---:|---:|---:|---:|
| **0 (off)** | 0.978 | **0.814** | **0.774** |
| 0.05 | 0.978 | 0.803 | 0.770 |
| 0.10 | 0.978 | 0.800 | 0.764 |
| 0.12 | 0.978 | 0.787 | 0.758 |
| 0.20 | 0.978 | 0.765 | 0.744 |
| 0.50 | 0.978 | 0.705 | 0.699 |

Monotonically worse. No half-life recovers it either -- at strength 0.05 the
best of 90/180/365/730/1825 days is MRR 0.804, and at 0.12 it is 0.804:

| strength | 90 d | 180 d | 365 d | 730 d | 1825 d |
|---:|---:|---:|---:|---:|---:|
| 0.05 | 0.799 | 0.804 | 0.804 | 0.804 | 0.803 |
| 0.12 | 0.786 | 0.787 | 0.800 | 0.804 | 0.804 |

(MRR; off is 0.814.)

## Why, and why the eval is right

At strength 0.12, 13 of the 45 cases move. The four biggest losses are all
the same shape, and they are real regressions rather than an artefact of how
the set was labelled:

| case | question | what happened |
|---|---|---|
| q005 | When did Jason start consulting for Kagi? | `/r/`, which literally says "Kagi.com (consulting) *June 2025 - Present*", drops below a blog post about a Hugo module |
| q011 | Where is Jason located? | `/about/` drops below `/posts/2026/lastfm-scrobbler/` |
| q018 | What programming languages does Jason know? | `/skills/programming-languages/` drops below `/posts/2026/lastfm-scrobbler/` |
| q042 | has jason ever spoke anywhere or given talks | the two talk write-ups drop below `/posts/2026/lastfm-scrobbler/` |

The corpus is a personal site: the pages that answer factual questions about
a person are the evergreen reference pages (`/about/`, `/skills/*`, the two
resume pages), and they are either undated or carry an old nominal date. The
newest post outranks them on any query it matches loosely at all --
`/posts/2026/lastfm-scrobbler/` alone displaces a correct answer in four of
the losing cases. The two gains (q001 +0.067, q021 +0.087) do not pay for it.

A near-tie-only variant does not help either: in q005 the correct page leads
by 0.4 % (0.0456 vs 0.0454), so any rule strong enough to reorder near-ties
reorders that one the wrong way.

## What it does to AnthroSim

The second labelled set (35 questions, `--cms html --include-noindex` over
the rendered site) is unmoved at every strength from 0 to 0.3: Hit@10 1.000,
MRR 0.732, nDCG@10 0.722 throughout.

Not because the boost is gentle there -- because it never fires. The `html`
parser reads rendered pages and does not extract a date, so every chunk in
that index is undated, `newest` is `None`, and no spec is baked at all
(`eddie index` says so: "Recency boost: off (nothing in this content has a
date)"). Any site indexed with `--cms html` gets nothing from this feature
until dates are parsed out of the markup.

## The gate: it only applies to browse queries

Both sets above are question sets. That was the flaw in the first round of
tuning: "when did Jason start at Kagi" has one right answer and it is
whichever page states the fact, however old, so a date can only do damage.
"java" is a different animal -- it names a topic, has no single right
answer, and of two pages that mention Java about equally the recent one is
usually the one worth reading.

So the boost is gated on `search::looks_like_question`, the same rule the
widget uses to decide whether to show a FAQ answer: a question mark, an
opening question word, or five or more words. A question gets no boost at
all.

That makes the question sets unaffected *by construction*, and the numbers
confirm it -- jason-grey.com's 45 cases score identically at every strength
tried, because all 45 are questions:

| strength (half-life 1460 d) | Hit@10 | MRR | nDCG@10 |
|---:|---:|---:|---:|
| 0 | 0.978 | 0.814 | 0.774 |
| 0.15 | 0.978 | 0.814 | 0.774 |
| 0.30 | 0.978 | 0.814 | 0.774 |
| 0.60 | 0.978 | 0.814 | 0.774 |

## Tuning on browse queries

The corpus spans 2006 to 2026, so the half-life has to be years. At
strength 0.15 and a four-year half-life, `java` on jason-grey.com:

| # | before | after |
|---|---|---|
| 1 | Google+ Java API Launched (2011) | Google+ Java API Launched (2011) |
| 2 | Algorithms & Techniques (undated) | Web and API - Enterprise Rust (2023) |
| 3 | Watching Tech Trends (2009) | Algorithms & Techniques (undated) |
| 4 | Web and API - Enterprise Rust (2023) | Watching Tech Trends (2009) |
| 5 | Search (undated) | Eddie: Hybrid Search (2026) |
| 6 | Object Oriented Programming (2006) | Core Components - Enterprise Rust (2023) |

The most on-topic page -- a post that is literally about a Java API -- keeps
first place, while the 2009 and 2006 posts that merely mention Java are
pushed down and out. That is the intended shape: demote stale tangential
matches, do not overrule topical ones.

Stronger settings overshoot. At 0.35 the 2011 Java post falls to third
behind two Rust posts, which is worse: they mention Java in passing and it
is *about* Java. `rust`, `ai`, `python` and `hugo` barely move at any
setting, because their best matches are already recent.

Defaults: **strength 0.15, half-life 1460 days**, on, gated.

## What this does not fix

`java` is flat: the top eight results span 0.0439 to 0.0380, a 13 % spread
across pages of very different relevance, and
`/skills/programming-languages/` -- the page that actually lists Java among
the languages he knows -- sits eighth. A single common term gives all three
arms a weak, similar signal and RRF flattens what is left, so the order
among near-ties is decided by whatever tips the balance, age included.

The recency boost improves that ordering but does not address the cause. The
real fix for broad single-term queries is upstream in retrieval, not in the
final sort.

## Decision

Shipped **on by default** at strength 0.15 with a four-year half-life,
gated to browse-style queries. `--no-recency` leaves it out of the index
entirely; `--recency 0` does the same thing at query time. An index built
before this exists carries no spec and ranks on relevance alone.

On by default is defensible only because of the gate: question answering
cannot be affected, and the browse-query change demotes stale tangential
matches without overruling topical ones. Without the gate it was worse on
every setting tried, which is what the first half of this page measured.

Two things still worth doing, neither of them this change:

- **Dates from `--cms html`.** A rendered-HTML corpus has no dates to rank
  by, so the feature is inert on any site indexed that way.
- **Broad single-term retrieval.** See "What this does not fix": the
  scores for `java` are nearly flat and the most relevant page is eighth.
  That is where the original complaint really comes from.
