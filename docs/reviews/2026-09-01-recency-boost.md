<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Recency in the final ranking: what it does to answer quality

Date: 2026-09-01. Asked for as "an optional/on by default boost in the final
ranking for newer pages", with the explicit condition "make sure our answers
are still accurate". This is the accuracy half.

## What was built

`manifest.recency` (a [`RecencySpec`]) carries a `strength` and a
`half_life_days`, baked in by `eddie index --recency` / `--recency-half-life`
and left out entirely by `--no-recency`. `group_pages` multiplies each page's
fused score by

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

## What it does to jason-grey.com

45 labelled questions, graded nDCG, the site's own index and weights
(1.2/0.8/0.6), half-life 240 days:

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

## Decision

Shipped, tunable, and **off unless a site asks for it**. `eddie index`
without `--recency` bakes no spec, and an index that carries no spec ranks on
relevance alone. A site whose freshness genuinely matters -- a news or
release-notes corpus, where the 2019 answer is wrong rather than merely old
-- turns it on with `--recency 0.12` and tunes it with
`eddie eval --recency`.

This is deliberately not what was asked for ("on by default"). The same
request asked for the accuracy check, and the accuracy check says one of the
two corpora is made worse at every setting tried and the other cannot use
the feature at all. The knob, the sweep and this page are here so that
judgement can be revisited per site rather than guessed at.

Two things that would change the answer, neither of them this change:

- **Dates from `--cms html`.** Half the reason the feature looks useless is
  that a rendered-HTML corpus has no dates to rank by.
- **A signal other than age.** What went wrong on jason-grey.com is that a
  blog post outranks a reference page, and "newer" is a poor proxy for
  "the kind of page that answers this question". A page-type or
  query-intent signal would address the original complaint without
  demoting `/about/`.
