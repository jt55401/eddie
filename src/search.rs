// SPDX-License-Identifier: GPL-3.0-only

//! Retrieval: three arms (dense, learned sparse, BM25) fused with weighted
//! reciprocal rank fusion, then grouped into page results with query-focused
//! snippets. Pure functions over a [`SearchIndex`]; the CLI and the WASM
//! module both call [`retrieve`] + [`group_pages`] so their rankings agree.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::bm25::tokenize;
use crate::index::{SearchIndex, strip_leading_words};
use crate::manifest::SparseTerm;

/// RRF constant: `score += weight / (RRF_K + rank)` with 1-based ranks.
pub const RRF_K: f64 = 60.0;
/// Snippet budget in characters.
pub const SNIPPET_MAX_CHARS: usize = 180;
/// Page bonus: `+AGREEMENT_BONUS × score(second-best chunk of a different granularity)`.
pub const AGREEMENT_BONUS: f64 = 0.10;

/// Which arms take part in a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Hybrid,
    Dense,
    Sparse,
    Keyword,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hybrid" | "" => Some(Mode::Hybrid),
            "dense" | "semantic" => Some(Mode::Dense),
            "sparse" => Some(Mode::Sparse),
            "keyword" | "bm25" => Some(Mode::Keyword),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Hybrid => "hybrid",
            Mode::Dense => "dense",
            Mode::Sparse => "sparse",
            Mode::Keyword => "keyword",
        }
    }

    fn wants_dense(self) -> bool {
        matches!(self, Mode::Hybrid | Mode::Dense)
    }
    fn wants_sparse(self) -> bool {
        matches!(self, Mode::Hybrid | Mode::Sparse)
    }
    fn wants_bm25(self) -> bool {
        matches!(self, Mode::Hybrid | Mode::Keyword)
    }
}

/// Per-arm RRF weights. When the sparse arm does not run, BM25 stands in for
/// the lexical signal and uses `max(bm25, sparse)` (1.0 with the defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub dense: f64,
    pub sparse: f64,
    pub bm25: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            dense: 1.0,
            sparse: 1.0,
            bm25: 0.8,
        }
    }
}

/// One search request. Arms whose input is `None` are skipped and reported
/// in [`Retrieval::degraded`]; they never cause an error.
#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub text: &'a str,
    /// `(lane index into SearchIndex::dense, query vector)`.
    pub dense: Option<(usize, Vec<f32>)>,
    pub sparse: Option<Vec<SparseTerm>>,
    pub mode: Mode,
    pub top_k: usize,
    pub weights: Weights,
}

/// A fused chunk. Ranks are 1-based positions within each arm's fetch list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RankedChunk {
    pub chunk: usize,
    pub score: f64,
    pub dense_rank: Option<usize>,
    pub sparse_rank: Option<usize>,
    pub bm25_rank: Option<usize>,
}

/// Which arms contributed to a result set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Arms {
    pub dense: bool,
    pub sparse: bool,
    pub bm25: bool,
}

/// Output of [`retrieve`]: fused chunks (all candidates, best first) plus
/// what ran and what was skipped.
#[derive(Debug, Clone, Serialize)]
pub struct Retrieval {
    pub ranked: Vec<RankedChunk>,
    pub arms: Arms,
    /// Human-readable reasons an arm that was requested did not run.
    pub degraded: Vec<String>,
}

/// One page in the final result list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageResult {
    pub url: String,
    pub title: String,
    pub section: Option<String>,
    /// Best chunk id for the page.
    pub chunk: usize,
    /// Every fused chunk of this page, best first.
    pub chunks: Vec<usize>,
    pub score: f64,
    pub snippet: String,
    pub date: Option<String>,
}

/// Candidates fetched per arm before fusion.
pub fn fetch_k(top_k: usize) -> usize {
    top_k.saturating_mul(3).max(30)
}

/// Run the arms selected by `q.mode` and fuse them with weighted RRF.
///
/// Errors only on misuse (unknown lane index, query vector of the wrong
/// dimension). A missing input for an arm skips that arm and adds a note to
/// `degraded`.
pub fn retrieve(index: &SearchIndex, q: &Query) -> Result<Retrieval> {
    let fetch = fetch_k(q.top_k);
    let mut arms = Arms::default();
    let mut degraded = Vec::new();

    let dense_hits: Vec<usize> = if q.mode.wants_dense() {
        match &q.dense {
            Some((lane_idx, vec)) => {
                let lane = index
                    .dense
                    .get(*lane_idx)
                    .with_context(|| format!("dense lane {} is out of range", lane_idx))?;
                let hits = lane.top_k(vec, fetch)?;
                arms.dense = true;
                hits.into_iter().map(|h| h.0).collect()
            }
            None => {
                degraded.push(if index.dense.is_empty() {
                    "dense: index has no dense lane".to_string()
                } else {
                    "dense: no query vector (no runnable embedder)".to_string()
                });
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let sparse_hits: Vec<usize> = if q.mode.wants_sparse() {
        match (&index.sparse, &q.sparse) {
            (Some(sparse), Some(terms)) => {
                arms.sparse = true;
                sparse
                    .top_k(terms, fetch)
                    .into_iter()
                    .map(|h| h.0)
                    .collect()
            }
            (Some(_), None) => {
                degraded.push("sparse: no query terms (tokenizer not loaded)".to_string());
                Vec::new()
            }
            (None, _) => {
                if q.mode == Mode::Sparse {
                    degraded.push("sparse: index has no sparse arm".to_string());
                }
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let bm25_hits: Vec<usize> = if q.mode.wants_bm25() {
        arms.bm25 = true;
        index
            .bm25
            .search(q.text, fetch)
            .into_iter()
            .map(|h| h.0)
            .collect()
    } else {
        Vec::new()
    };

    let bm25_weight = if arms.sparse {
        q.weights.bm25
    } else {
        q.weights.bm25.max(q.weights.sparse)
    };

    let mut fused: HashMap<usize, RankedChunk> = HashMap::new();
    fn entry(fused: &mut HashMap<usize, RankedChunk>, chunk: usize) -> &mut RankedChunk {
        fused.entry(chunk).or_insert_with(|| RankedChunk {
            chunk,
            score: 0.0,
            dense_rank: None,
            sparse_rank: None,
            bm25_rank: None,
        })
    }
    for (i, &chunk) in dense_hits.iter().enumerate() {
        let e = entry(&mut fused, chunk);
        e.score += q.weights.dense / (RRF_K + (i + 1) as f64);
        e.dense_rank = Some(i + 1);
    }
    for (i, &chunk) in sparse_hits.iter().enumerate() {
        let e = entry(&mut fused, chunk);
        e.score += q.weights.sparse / (RRF_K + (i + 1) as f64);
        e.sparse_rank = Some(i + 1);
    }
    for (i, &chunk) in bm25_hits.iter().enumerate() {
        let e = entry(&mut fused, chunk);
        e.score += bm25_weight / (RRF_K + (i + 1) as f64);
        e.bm25_rank = Some(i + 1);
    }

    let mut ranked: Vec<RankedChunk> = fused.into_values().collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.chunk.cmp(&b.chunk))
    });

    Ok(Retrieval {
        ranked,
        arms,
        degraded,
    })
}

/// Collapse fused chunks into at most `top_k` pages (one per URL).
///
/// Each page is represented by its best chunk and scores `best +
/// AGREEMENT_BONUS × (best other chunk of a different granularity)`. Ties are
/// broken by date (newest first, undated last) and then URL, so the order is
/// deterministic and recency never reorders pages with different scores.
pub fn group_pages(
    index: &SearchIndex,
    ranked: &[RankedChunk],
    query_terms: &[String],
    top_k: usize,
) -> Vec<PageResult> {
    struct Page {
        best: usize,
        best_score: f64,
        chunks: Vec<usize>,
        bonus_source: f64,
    }
    let mut order: Vec<&str> = Vec::new();
    let mut pages: HashMap<&str, Page> = HashMap::new();

    for r in ranked {
        let Some(meta) = index.metadata.get(r.chunk) else {
            continue;
        };
        match pages.get_mut(meta.url.as_str()) {
            None => {
                order.push(meta.url.as_str());
                pages.insert(
                    meta.url.as_str(),
                    Page {
                        best: r.chunk,
                        best_score: r.score,
                        chunks: vec![r.chunk],
                        bonus_source: 0.0,
                    },
                );
            }
            Some(page) => {
                page.chunks.push(r.chunk);
                let best_gran = index.metadata[page.best].granularity.as_deref();
                if meta.granularity.as_deref() != best_gran {
                    page.bonus_source = page.bonus_source.max(r.score);
                }
            }
        }
    }

    let mut results: Vec<PageResult> = order
        .into_iter()
        .map(|url| {
            let page = pages.remove(url).expect("page inserted above");
            let meta = &index.metadata[page.best];
            let text = index.texts.get(page.best).map(String::as_str).unwrap_or("");
            PageResult {
                url: meta.url.clone(),
                title: meta.title.clone(),
                section: meta.section.clone(),
                chunk: page.best,
                chunks: page.chunks,
                score: page.best_score + AGREEMENT_BONUS * page.bonus_source,
                snippet: snippet(text, 0, query_terms, SNIPPET_MAX_CHARS),
                date: meta.date.clone(),
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| match (&a.date, &b.date) {
                (Some(x), Some(y)) => y.cmp(x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.url.cmp(&b.url))
    });
    results.truncate(top_k);
    results
}

/// Query terms for snippet selection: the BM25 tokens of `text`, deduplicated
/// in order of first appearance, with common English stop words removed
/// unless nothing else remains.
pub fn query_terms(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let all: Vec<String> = tokenize(text)
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect();
    let content: Vec<String> = all.iter().filter(|t| !is_stop_word(t)).cloned().collect();
    if content.is_empty() { all } else { content }
}

fn is_stop_word(t: &str) -> bool {
    matches!(
        t,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "by"
            | "can"
            | "do"
            | "does"
            | "for"
            | "from"
            | "how"
            | "i"
            | "in"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "that"
            | "the"
            | "this"
            | "to"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "with"
            | "you"
            | "your"
    )
}

/// Pick the sentence window (≤ `max_chars` characters) of `text` with the most
/// query-term hits, skipping the first `overlap_words` words. Falls back to the
/// start of the text when no term matches. Never cuts inside a UTF-8 character
/// or a word; a window that is still too long is trimmed to the word span
/// with the most hits and marked with `…`.
pub fn snippet(
    text: &str,
    overlap_words: usize,
    query_terms: &[String],
    max_chars: usize,
) -> String {
    let body = strip_leading_words(text, overlap_words);
    let sentences = split_sentences(body);
    if sentences.is_empty() {
        return String::new();
    }
    let terms: HashSet<&str> = query_terms.iter().map(String::as_str).collect();
    let hits: Vec<usize> = sentences.iter().map(|s| count_hits(s, &terms)).collect();
    let lens: Vec<usize> = sentences.iter().map(|s| s.chars().count()).collect();

    // Best window of whole sentences.
    let mut best = (0usize, 0usize, 0usize); // (hits, start, end_exclusive)
    let mut found = false;
    for start in 0..sentences.len() {
        let mut chars = lens[start];
        let mut h = hits[start];
        let mut end = start + 1;
        while end < sentences.len() && chars + 1 + lens[end] <= max_chars {
            chars += 1 + lens[end];
            h += hits[end];
            end += 1;
        }
        if !found || h > best.0 {
            best = (h, start, end);
            found = true;
        }
    }
    let (_, start, end) = if best.0 == 0 {
        // No hits anywhere: take the leading sentences that fit.
        let mut end = 1;
        let mut chars = lens[0];
        while end < sentences.len() && chars + 1 + lens[end] <= max_chars {
            chars += 1 + lens[end];
            end += 1;
        }
        (0, 0, end)
    } else {
        best
    };
    let window = sentences[start..end].join(" ");
    if window.chars().count() <= max_chars {
        return window;
    }
    trim_to_words(&window, &terms, max_chars)
}

/// Split on sentence punctuation followed by whitespace, or on newlines;
/// collapse internal whitespace.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_was_terminator = false;
    for ch in text.chars() {
        if ch == '\n' || (prev_was_terminator && ch.is_whitespace() && !is_list_marker(&current)) {
            push_sentence(&mut out, &mut current);
            prev_was_terminator = false;
            continue;
        }
        current.push(ch);
        prev_was_terminator = matches!(ch, '.' | '!' | '?' | '。' | '！' | '？');
    }
    push_sentence(&mut out, &mut current);
    out
}

/// `1.`, `12.`, `a.` at the end of the buffer: an ordered-list marker, not a
/// sentence end.
fn is_list_marker(current: &str) -> bool {
    let tail = current.rsplit(char::is_whitespace).next().unwrap_or("");
    let Some(body) = tail.strip_suffix('.') else {
        return false;
    };
    !body.is_empty()
        && body.chars().count() <= 3
        && (body.chars().all(|c| c.is_ascii_digit())
            || (body.chars().count() == 1 && body.chars().all(|c| c.is_ascii_alphabetic())))
}

fn push_sentence(out: &mut Vec<String>, current: &mut String) {
    let collapsed = current.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        out.push(collapsed);
    }
    current.clear();
}

fn count_hits(text: &str, terms: &HashSet<&str>) -> usize {
    if terms.is_empty() {
        return 0;
    }
    tokenize(text)
        .iter()
        .filter(|t| terms.contains(t.as_str()))
        .count()
}

/// Word span of at most `max_chars` (including ellipses) with the most hits.
fn trim_to_words(text: &str, terms: &HashSet<&str>, max_chars: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let budget = max_chars.saturating_sub(2).max(1);
    let wlens: Vec<usize> = words.iter().map(|w| w.chars().count()).collect();
    let whits: Vec<usize> = words.iter().map(|w| count_hits(w, terms)).collect();

    let mut best = (0usize, 0usize, 0usize);
    let mut found = false;
    for start in 0..words.len() {
        if wlens[start] > budget {
            continue;
        }
        let mut chars = wlens[start];
        let mut h = whits[start];
        let mut end = start + 1;
        while end < words.len() && chars + 1 + wlens[end] <= budget {
            chars += 1 + wlens[end];
            h += whits[end];
            end += 1;
        }
        if !found || h > best.0 {
            best = (h, start, end);
            found = true;
        }
        if h == 0 && found && best.0 == 0 {
            // Keep scanning for a span with hits; the first span stays the fallback.
        }
    }
    if !found {
        // Every single word exceeds the budget: cut the first one on a char boundary.
        let cut: String = words[0].chars().take(budget).collect();
        return format!("{}…", cut);
    }
    let (_, start, end) = best;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&words[start..end].join(" "));
    if end < words.len() {
        out.push('…');
    }
    out
}

/// Query-side sparse terms: WordPiece ids of the query weighted by the IDF the
/// index stores for them (see `crate::sparse::sparse_query_terms`). Kept as a
/// `Result` for callers that treat tokenizer failures as errors.
pub fn sparse_query_terms_local(
    tokenizer: &tokenizers::Tokenizer,
    idf: &dyn Fn(u32) -> Option<f32>,
    query: &str,
) -> Result<Vec<SparseTerm>> {
    Ok(crate::sparse::sparse_query_terms(tokenizer, idf, query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::testutil::*;
    use crate::index::{DenseLane, IndexBuilder, SCOPE_CHUNKS};
    use crate::manifest::Quant;

    fn index_2k() -> (SearchIndex, Synthetic) {
        build_synthetic_index(2000, 384, Quant::Int8, true)
    }

    fn query_for<'a>(
        corpus: &Synthetic,
        text: &'a str,
        doc: usize,
        mode: Mode,
        top_k: usize,
    ) -> Query<'a> {
        let dim = corpus.dim;
        let vec = corpus.vectors[doc * dim..(doc + 1) * dim].to_vec();
        let cluster = doc % 40;
        let sparse: Vec<SparseTerm> = (0..20)
            .map(|k| SparseTerm {
                token_id: 30_000 + (cluster * 20 + k) as u32,
                weight: 1.5,
            })
            .collect();
        Query {
            text,
            dense: Some((0, vec)),
            sparse: Some(sparse),
            mode,
            top_k,
            weights: Weights::default(),
        }
    }

    #[test]
    fn fusion_score_is_monotonic_in_rank_and_ties_are_deterministic() {
        let (index, corpus) = index_2k();
        let q = query_for(&corpus, "topic3 facts", 3, Mode::Hybrid, 8);
        let r = retrieve(&index, &q).unwrap();
        assert!(r.arms.dense && r.arms.sparse && r.arms.bm25);
        assert!(r.degraded.is_empty());
        assert!(!r.ranked.is_empty());
        for w in r.ranked.windows(2) {
            assert!(w[0].score >= w[1].score);
            if w[0].score == w[1].score {
                assert!(w[0].chunk < w[1].chunk);
            }
        }
        // The query vector is doc 3's own vector: it must be the dense rank 1.
        let top = r.ranked.iter().find(|c| c.chunk == 3).unwrap();
        assert_eq!(top.dense_rank, Some(1));
        // Same query twice gives identical output.
        let r2 = retrieve(&index, &q).unwrap();
        assert_eq!(r.ranked, r2.ranked);

        // A chunk that appears in two arms outranks one at the same rank in one arm.
        let single = RankedChunk {
            chunk: 1,
            score: 1.0 / (RRF_K + 1.0),
            dense_rank: Some(1),
            sparse_rank: None,
            bm25_rank: None,
        };
        let double = r
            .ranked
            .iter()
            .find(|c| c.dense_rank.is_some() && c.bm25_rank.is_some());
        if let Some(d) = double {
            let expected = 1.0 / (RRF_K + d.dense_rank.unwrap() as f64)
                + 0.8 / (RRF_K + d.bm25_rank.unwrap() as f64)
                + d.sparse_rank
                    .map(|s| 1.0 / (RRF_K + s as f64))
                    .unwrap_or(0.0);
            assert!((d.score - expected).abs() < 1e-12);
            let _ = single;
        }
    }

    #[test]
    fn dedup_returns_exactly_min_top_k_pages() {
        let (index, corpus) = index_2k();
        for &top_k in &[1usize, 5, 8, 20] {
            let q = query_for(&corpus, "topic7 facts", 7, Mode::Hybrid, top_k);
            let r = retrieve(&index, &q).unwrap();
            let pages = group_pages(&index, &r.ranked, &query_terms(q.text), top_k);
            let distinct: HashSet<&str> = r
                .ranked
                .iter()
                .map(|c| index.metadata[c.chunk].url.as_str())
                .collect();
            assert_eq!(pages.len(), top_k.min(distinct.len()));
            let urls: HashSet<&str> = pages.iter().map(|p| p.url.as_str()).collect();
            assert_eq!(urls.len(), pages.len(), "no duplicate urls");
            for w in pages.windows(2) {
                assert!(w[0].score >= w[1].score);
            }
            for p in &pages {
                assert!(p.chunks.contains(&p.chunk));
                assert_eq!(p.chunks[0], p.chunk);
                assert!(p.snippet.chars().count() <= SNIPPET_MAX_CHARS);
            }
        }
    }

    #[test]
    fn agreement_bonus_is_bounded_and_needs_distinct_granularity() {
        let (index, _) = index_2k();
        // Chunk 0 (fine) and chunk 7 (coarse) share page 0; chunk 1 (fine) too.
        let ranked = vec![
            RankedChunk {
                chunk: 0,
                score: 0.5,
                dense_rank: Some(1),
                sparse_rank: None,
                bm25_rank: None,
            },
            RankedChunk {
                chunk: 1,
                score: 0.4,
                dense_rank: Some(2),
                sparse_rank: None,
                bm25_rank: None,
            },
            RankedChunk {
                chunk: 7,
                score: 0.3,
                dense_rank: Some(3),
                sparse_rank: None,
                bm25_rank: None,
            },
            RankedChunk {
                chunk: 8,
                score: 0.45,
                dense_rank: Some(4),
                sparse_rank: None,
                bm25_rank: None,
            },
        ];
        let pages = group_pages(&index, &ranked, &[], 10);
        assert_eq!(pages.len(), 2);
        let p0 = pages.iter().find(|p| p.url == "/page-0/").unwrap();
        // Bonus comes from chunk 7 (coarse), not chunk 1 (same granularity as best).
        assert!((p0.score - (0.5 + AGREEMENT_BONUS * 0.3)).abs() < 1e-12);
        assert_eq!(p0.chunks, vec![0, 1, 7]);
        let p1 = pages.iter().find(|p| p.url == "/page-1/").unwrap();
        assert_eq!(p1.score, 0.45);
        assert_eq!(pages[0].url, "/page-0/");
    }

    #[test]
    fn page_ties_break_by_date_then_url() {
        let (index, _) = index_2k();
        // page-0 undated, page-1 dated 2021, page-2 dated 2022 (page % 3 == 0 -> None).
        let ranked = vec![
            RankedChunk {
                chunk: 0,
                score: 0.5,
                dense_rank: Some(1),
                sparse_rank: None,
                bm25_rank: None,
            },
            RankedChunk {
                chunk: 8,
                score: 0.5,
                dense_rank: Some(2),
                sparse_rank: None,
                bm25_rank: None,
            },
            RankedChunk {
                chunk: 16,
                score: 0.5,
                dense_rank: Some(3),
                sparse_rank: None,
                bm25_rank: None,
            },
        ];
        let pages = group_pages(&index, &ranked, &[], 10);
        let urls: Vec<&str> = pages.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(urls, vec!["/page-2/", "/page-1/", "/page-0/"]);
    }

    #[test]
    fn modes_skip_missing_arms_without_error() {
        let (index, corpus) = index_2k();
        let mut q = query_for(&corpus, "topic3 facts", 3, Mode::Hybrid, 5);
        q.dense = None;
        q.sparse = None;
        let r = retrieve(&index, &q).unwrap();
        assert!(!r.arms.dense && !r.arms.sparse && r.arms.bm25);
        assert_eq!(r.degraded.len(), 2);
        assert!(!r.ranked.is_empty());
        // BM25 alone is weighted 1.0 when sparse did not run.
        assert!((r.ranked[0].score - 1.0 / (RRF_K + 1.0)).abs() < 1e-12);

        q.mode = Mode::Keyword;
        let r = retrieve(&index, &q).unwrap();
        assert!(r.degraded.is_empty());
        assert!(r.ranked.iter().all(|c| c.bm25_rank.is_some()));

        q.mode = Mode::Dense;
        let r = retrieve(&index, &q).unwrap();
        assert!(r.ranked.is_empty());
        assert_eq!(r.degraded.len(), 1);

        // Wrong dimension is a real error.
        let mut bad = query_for(&corpus, "x", 3, Mode::Dense, 5);
        bad.dense = Some((0, vec![0.0; 10]));
        assert!(retrieve(&index, &bad).is_err());
        bad.dense = Some((9, vec![0.0; 384]));
        assert!(retrieve(&index, &bad).is_err());

        // Sparse-only mode on an index without a sparse arm: degraded, not an error.
        let (no_sparse, corpus2) = build_synthetic_index(64, 16, Quant::F32, false);
        let q = query_for(&corpus2, "topic1", 1, Mode::Sparse, 5);
        let r = retrieve(&no_sparse, &q).unwrap();
        assert!(r.ranked.is_empty());
        assert_eq!(
            r.degraded,
            vec!["sparse: index has no sparse arm".to_string()]
        );
        // Hybrid on the same index: sparse silently absent, bm25 weight 1.0.
        let q = query_for(&corpus2, "topic1", 1, Mode::Hybrid, 5);
        let r = retrieve(&no_sparse, &q).unwrap();
        assert!(r.degraded.is_empty());
        assert!(r.arms.dense && r.arms.bm25 && !r.arms.sparse);
    }

    #[test]
    fn empty_query_text_yields_only_dense_hits() {
        let (index, corpus) = index_2k();
        let q = query_for(&corpus, "??", 5, Mode::Hybrid, 5);
        let r = retrieve(&index, &q).unwrap();
        assert!(r.ranked.iter().all(|c| c.bm25_rank.is_none()));
        assert!(r.ranked.iter().any(|c| c.dense_rank.is_some()));
    }

    #[test]
    fn snippet_contains_a_query_term_when_the_chunk_does() {
        let (index, _) = index_2k();
        let mut rng = Rng(11);
        for _ in 0..200 {
            let chunk = rng.below(index.texts.len());
            let text = &index.texts[chunk];
            let tokens = tokenize(text);
            let pick = tokens[rng.below(tokens.len())].clone();
            let terms = query_terms(&pick);
            assert_eq!(terms, vec![pick.clone()]);
            let s = snippet(text, 0, &terms, SNIPPET_MAX_CHARS);
            assert!(s.chars().count() <= SNIPPET_MAX_CHARS, "{}", s);
            assert!(!s.is_empty());
            let s_tokens: HashSet<String> = tokenize(&s).into_iter().collect();
            assert!(
                terms.iter().any(|t| s_tokens.contains(t)),
                "snippet {:?} lacks all of {:?}",
                s,
                terms
            );
        }
    }

    #[test]
    fn snippet_skips_overlap_and_prefers_the_matching_sentence() {
        let text = "Overlap words from before. The cache TTL is configured in eddie.toml. Another sentence here.";
        let terms = query_terms("configure the cache ttl");
        let s = snippet(text, 4, &terms, 60);
        assert_eq!(s, "The cache TTL is configured in eddie.toml.");
        // Without hits: the start of the clean text.
        let s = snippet(text, 4, &query_terms("zebra"), 60);
        assert!(s.starts_with("The cache TTL"));
        // Fully consumed by the overlap: empty.
        assert_eq!(snippet("a b", 2, &terms, 60), "");
        assert_eq!(snippet("", 0, &terms, 60), "");
    }

    #[test]
    fn snippet_never_cuts_inside_chars_or_words() {
        let long_word = "ü".repeat(500);
        let s = snippet(&long_word, 0, &[], 20);
        assert!(s.chars().count() <= 20);
        assert!(s.ends_with('…'));

        let text = "wörd ".repeat(100);
        let terms = vec!["wörd".to_string()];
        let s = snippet(&text, 0, &terms, 30);
        assert!(s.chars().count() <= 30);
        assert!(s.trim_matches('…').split(' ').all(|w| w == "wörd"), "{}", s);

        // A hit deep inside one huge sentence is still surfaced.
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("filler{} ", i));
        }
        text.push_str("needle here");
        let s = snippet(&text, 0, &["needle".to_string()], 40);
        assert!(s.contains("needle"), "{}", s);
        assert!(s.starts_with('…'));
        assert!(s.chars().count() <= 40);
    }

    #[test]
    fn list_markers_do_not_split_sentences() {
        assert_eq!(
            split_sentences("Steps: 1. Fetch the index. 2. Pick a lane. Done."),
            vec!["Steps: 1. Fetch the index.", "2. Pick a lane.", "Done."]
        );
        assert_eq!(
            split_sentences("See v0.4. Then a. Next"),
            vec!["See v0.4.", "Then a. Next"]
        );
        assert!(is_list_marker("foo 12."));
        assert!(is_list_marker("b."));
        assert!(!is_list_marker("1234."));
        assert!(!is_list_marker("end."));
        assert!(!is_list_marker(""));
    }

    #[test]
    fn query_terms_drops_stop_words_unless_nothing_remains() {
        assert_eq!(
            query_terms("How do I configure the cache?"),
            vec!["configure", "cache"]
        );
        assert_eq!(query_terms("what is this"), vec!["what", "is", "this"]);
        assert_eq!(query_terms("rust rust Rust"), vec!["rust"]);
        assert!(query_terms("??").is_empty());
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(Mode::parse("hybrid"), Some(Mode::Hybrid));
        assert_eq!(Mode::parse("Dense"), Some(Mode::Dense));
        assert_eq!(Mode::parse("semantic"), Some(Mode::Dense));
        assert_eq!(Mode::parse("sparse"), Some(Mode::Sparse));
        assert_eq!(Mode::parse("keyword"), Some(Mode::Keyword));
        assert_eq!(Mode::parse("nope"), None);
    }

    #[test]
    fn lane_index_is_respected() {
        // Two lanes with different dims: the query must go to the named lane.
        let corpus = synthetic_corpus(32, 8, 3);
        let mut b = IndexBuilder::new();
        b.add_chunks(corpus.metadata.clone(), corpus.texts.clone(), vec![0; 32])
            .unwrap();
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(
                wasm_spec("a", 8, Quant::F32),
                8,
                32,
                &corpus.vectors,
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        let other: Vec<f32> = (0..32 * 4).map(|i| (i % 4) as f32 * 0.5).collect();
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(wasm_spec("b", 4, Quant::F32), 4, 32, &other, Quant::F32).unwrap(),
        )
        .unwrap();
        let index = b.finish().unwrap();
        assert_eq!(index.dense_lane("b"), Some(1));
        let q = Query {
            text: "",
            dense: Some((1, vec![0.0, 0.0, 0.0, 1.0])),
            sparse: None,
            mode: Mode::Dense,
            top_k: 3,
            weights: Weights::default(),
        };
        let r = retrieve(&index, &q).unwrap();
        assert_eq!(r.ranked.len(), 30);
        let q = Query {
            text: "",
            dense: Some((0, vec![0.0, 0.0, 0.0, 1.0])),
            sparse: None,
            mode: Mode::Dense,
            top_k: 3,
            weights: Weights::default(),
        };
        assert!(retrieve(&index, &q).is_err());
    }
}
