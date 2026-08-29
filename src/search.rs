// SPDX-License-Identifier: GPL-3.0-only

//! Retrieval: three arms (dense, learned sparse, BM25) fused with weighted
//! reciprocal rank fusion, then grouped into page results with query-focused
//! snippets. Pure functions over a [`SearchIndex`]; the CLI and the WASM
//! module both call [`retrieve`] + [`group_pages`] so their rankings agree.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::bm25::tokenize;
use crate::index::{SearchIndex, query_vector_problem, strip_leading_words};
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

impl Weights {
    /// Parse `"dense,sparse,bm25"` (three finite, non-negative floats, not
    /// all zero) as written for `--weights`.
    pub fn parse(s: &str) -> Result<Weights> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 3 {
            anyhow::bail!(
                "weights must be three comma-separated numbers dense,sparse,bm25 (got {:?})",
                s
            );
        }
        let mut vals = [0.0f64; 3];
        for (v, (part, name)) in vals
            .iter_mut()
            .zip(parts.iter().zip(["dense", "sparse", "bm25"]))
        {
            let x: f64 = part
                .parse()
                .with_context(|| format!("{} weight {:?} is not a number", name, part))?;
            if !x.is_finite() || x < 0.0 {
                anyhow::bail!(
                    "{} weight must be a finite number >= 0 (got {})",
                    name,
                    part
                );
            }
            *v = x;
        }
        if vals.iter().all(|&v| v == 0.0) {
            anyhow::bail!("at least one weight must be > 0");
        }
        Ok(Weights {
            dense: vals[0],
            sparse: vals[1],
            bm25: vals[2],
        })
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
    /// Candidates fetched per arm; `None` means [`fetch_k`]`(top_k)`.
    pub fetch_k: Option<usize>,
    /// RRF constant; `None` means [`RRF_K`].
    pub rrf_k: Option<f64>,
}

impl Default for Query<'_> {
    /// Hybrid, top 8, default weights, no arm inputs, no overrides.
    fn default() -> Self {
        Self {
            text: "",
            dense: None,
            sparse: None,
            mode: Mode::Hybrid,
            top_k: 8,
            weights: Weights::default(),
            fetch_k: None,
            rrf_k: None,
        }
    }
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
/// `degraded`; so does a query vector that cannot rank anything (NaN or all
/// zeros) or a sparse query with no terms, since running those arms would
/// hand RRF credit to arbitrary chunks.
pub fn retrieve(index: &SearchIndex, q: &Query) -> Result<Retrieval> {
    let fetch = q
        .fetch_k
        .filter(|&f| f > 0)
        .unwrap_or_else(|| fetch_k(q.top_k));
    let rrf_k = q
        .rrf_k
        .filter(|k| k.is_finite() && *k >= 0.0)
        .unwrap_or(RRF_K);
    let mut arms = Arms::default();
    let mut degraded = Vec::new();

    let dense_hits: Vec<usize> = if q.mode.wants_dense() {
        match &q.dense {
            Some((lane_idx, vec)) => {
                let lane = index
                    .dense
                    .get(*lane_idx)
                    .with_context(|| format!("dense lane {} is out of range", lane_idx))?;
                if vec.len() != lane.dim {
                    anyhow::bail!(
                        "query vector has {} dims but lane {:?} has {}",
                        vec.len(),
                        lane.spec.id,
                        lane.dim
                    );
                }
                match query_vector_problem(vec) {
                    Some(problem) => {
                        degraded.push(format!(
                            "dense: query vector for lane {:?} {}",
                            lane.spec.id, problem
                        ));
                        Vec::new()
                    }
                    None => {
                        let hits = lane.top_k(vec, fetch)?;
                        arms.dense = true;
                        hits.into_iter().map(|h| h.0).collect()
                    }
                }
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
            (Some(_), Some(terms)) if terms.is_empty() => {
                // Nothing in the query survived IDF lookup: the arm did not
                // run, so BM25 keeps the full lexical weight below.
                degraded.push("sparse: query has no terms in the index vocabulary".to_string());
                Vec::new()
            }
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
        e.score += q.weights.dense / (rrf_k + (i + 1) as f64);
        e.dense_rank = Some(i + 1);
    }
    for (i, &chunk) in sparse_hits.iter().enumerate() {
        let e = entry(&mut fused, chunk);
        e.score += q.weights.sparse / (rrf_k + (i + 1) as f64);
        e.sparse_rank = Some(i + 1);
    }
    for (i, &chunk) in bm25_hits.iter().enumerate() {
        let e = entry(&mut fused, chunk);
        e.score += bm25_weight / (rrf_k + (i + 1) as f64);
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

// ---------------------------------------------------------------------------
// QA ranking
// ---------------------------------------------------------------------------

/// Share of the QA score that comes from the dense cosine.
pub const QA_DENSE_WEIGHT: f64 = 0.6;
/// Share of the QA score that comes from the lexical signal (overlap + BM25).
pub const QA_LEXICAL_WEIGHT: f64 = 0.4;
/// A hit is `confident` when `score >= QA_CONFIDENT_SCORE` and either
/// `overlap >= QA_CONFIDENT_OVERLAP` or `dense >= QA_CONFIDENT_DENSE`.
pub const QA_CONFIDENT_SCORE: f64 = 0.55;
pub const QA_CONFIDENT_OVERLAP: f64 = 0.34;
pub const QA_CONFIDENT_DENSE: f64 = 0.80;

/// One QA entry ranked by [`rank_qa`].
///
/// ```text
/// lexical = 0.5 · overlap + 0.5 · bm25_norm
/// score   = 0.6 · dense + 0.4 · lexical
/// ```
///
/// * `dense`: cosine from the qa dense lane (0 when the entry was not among
///   the dense candidates handed in).
/// * `overlap`: matched query terms / query terms, over [`qa_overlap_terms`]
///   (BM25 tokens minus stop words and auxiliaries), matched against the
///   entry's question and answer.
/// * `bm25_norm`: the entry's BM25 score over `question + " " + answer`
///   divided by the best BM25 score for the query (0 when the entry is not
///   in the BM25 candidate list); `bm25_rank` is its 1-based rank there.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QaHit {
    pub id: usize,
    pub score: f64,
    pub dense: f64,
    pub overlap: f64,
    pub bm25_rank: Option<usize>,
    pub confident: bool,
}

/// How many dense candidates to fetch from the qa lane before calling
/// [`rank_qa`] for `k` results: enough that lexical evidence can promote an
/// entry the dense arm ranked a few places down.
pub fn qa_fetch_k(k: usize) -> usize {
    k.saturating_mul(4).max(20)
}

/// Rank the QA entries of `index` for `query_text`.
///
/// `dense_hits` are `(entry id, cosine)` pairs from the qa dense lane
/// (see [`qa_fetch_k`]); pass an empty slice when no query vector exists and
/// the ranking is lexical only (such hits are never `confident`). Candidates
/// are the union of `dense_hits` and the BM25 top list; the result holds at
/// most `k` hits, best first, ties broken by entry id. See [`QaHit`] for the
/// formula.
pub fn rank_qa(
    index: &SearchIndex,
    query_text: &str,
    dense_hits: &[(usize, f32)],
    k: usize,
) -> Vec<QaHit> {
    if k == 0 || index.qa.is_empty() {
        return Vec::new();
    }
    let terms = qa_overlap_terms(query_text);
    let bm25_hits = index.qa_bm25().search(query_text, qa_fetch_k(k));
    let bm25_top = bm25_hits.first().map(|h| h.1).unwrap_or(0.0);

    let mut candidates: HashMap<usize, QaHit> = HashMap::new();
    for &(id, cos) in dense_hits {
        if id >= index.qa.len() {
            continue;
        }
        let e = candidates.entry(id).or_insert_with(|| blank_hit(id));
        e.dense = e.dense.max(cos.clamp(-1.0, 1.0) as f64);
    }
    let mut bm25_norm: HashMap<usize, f64> = HashMap::new();
    for (rank, &(id, score)) in bm25_hits.iter().enumerate() {
        let e = candidates.entry(id).or_insert_with(|| blank_hit(id));
        e.bm25_rank = Some(rank + 1);
        bm25_norm.insert(
            id,
            if bm25_top > 0.0 {
                score / bm25_top
            } else {
                0.0
            },
        );
    }

    let mut hits: Vec<QaHit> = candidates
        .into_values()
        .map(|mut h| {
            let entry = &index.qa[h.id];
            h.overlap = overlap_ratio(&terms, &format!("{} {}", entry.question, entry.answer));
            let bm25 = bm25_norm.get(&h.id).copied().unwrap_or(0.0);
            let lexical = 0.5 * h.overlap + 0.5 * bm25;
            h.score = QA_DENSE_WEIGHT * h.dense.max(0.0) + QA_LEXICAL_WEIGHT * lexical;
            h.confident = h.score >= QA_CONFIDENT_SCORE
                && (h.overlap >= QA_CONFIDENT_OVERLAP || h.dense >= QA_CONFIDENT_DENSE);
            h
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(k);
    hits
}

fn blank_hit(id: usize) -> QaHit {
    QaHit {
        id,
        score: 0.0,
        dense: 0.0,
        overlap: 0.0,
        bm25_rank: None,
        confident: false,
    }
}

/// Query terms for QA overlap: [`query_terms`] minus auxiliaries and other
/// function words that every question shares (`has`, `been`, `many`, ...),
/// unless nothing else remains.
pub fn qa_overlap_terms(text: &str) -> Vec<String> {
    let base = query_terms(text);
    let content: Vec<String> = base
        .iter()
        .filter(|t| !is_qa_function_word(t))
        .cloned()
        .collect();
    if content.is_empty() { base } else { content }
}

fn is_qa_function_word(t: &str) -> bool {
    matches!(
        t,
        "about"
            | "any"
            | "been"
            | "did"
            | "had"
            | "has"
            | "have"
            | "he"
            | "her"
            | "him"
            | "his"
            | "if"
            | "into"
            | "its"
            | "many"
            | "me"
            | "much"
            | "my"
            | "not"
            | "our"
            | "she"
            | "should"
            | "so"
            | "some"
            | "than"
            | "their"
            | "them"
            | "there"
            | "these"
            | "they"
            | "those"
            | "was"
            | "we"
            | "were"
            | "will"
            | "would"
    )
}

/// `|terms ∩ tokens(text)| / |terms|`, 0 when `terms` is empty.
fn overlap_ratio(terms: &[String], text: &str) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let present: HashSet<String> = tokenize(text).into_iter().collect();
    let matched = terms
        .iter()
        .filter(|t| present.contains(t.as_str()))
        .count();
    matched as f64 / terms.len() as f64
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
            ..Query::default()
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
    fn unusable_dense_query_degrades_instead_of_ranking_index_order() {
        let (index, corpus) = index_2k();
        for bad in [vec![f32::NAN; 384], vec![0.0; 384]] {
            let mut q = query_for(&corpus, "topic3 facts", 3, Mode::Hybrid, 5);
            q.dense = Some((0, bad));
            let r = retrieve(&index, &q).unwrap();
            assert!(!r.arms.dense && r.arms.sparse && r.arms.bm25);
            assert_eq!(r.degraded.len(), 1);
            assert!(
                r.degraded[0].starts_with("dense: query vector"),
                "{:?}",
                r.degraded
            );
            assert!(r.ranked.iter().all(|c| c.dense_rank.is_none()));
            assert!(!r.ranked.is_empty());
        }
        // Dense-only mode with such a vector: no results, one note, no error.
        let mut q = query_for(&corpus, "x", 3, Mode::Dense, 5);
        q.dense = Some((0, vec![0.0; 384]));
        let r = retrieve(&index, &q).unwrap();
        assert!(r.ranked.is_empty());
        assert_eq!(r.degraded.len(), 1);
    }

    #[test]
    fn empty_sparse_terms_count_as_not_run() {
        let (index, corpus) = index_2k();
        let mut q = query_for(&corpus, "topic3 facts", 3, Mode::Hybrid, 5);
        q.sparse = Some(Vec::new());
        let r = retrieve(&index, &q).unwrap();
        assert!(r.arms.dense && !r.arms.sparse && r.arms.bm25);
        assert_eq!(
            r.degraded,
            vec!["sparse: query has no terms in the index vocabulary".to_string()]
        );
        // BM25 stands in at weight 1.0, exactly as when the tokenizer is missing.
        let bm25_only = r
            .ranked
            .iter()
            .find(|c| c.bm25_rank.is_some() && c.dense_rank.is_none())
            .expect("a bm25-only chunk");
        let expected = 1.0 / (RRF_K + bm25_only.bm25_rank.unwrap() as f64);
        assert!((bm25_only.score - expected).abs() < 1e-12);
        q.sparse = None;
        let r2 = retrieve(&index, &q).unwrap();
        assert_eq!(r.ranked, r2.ranked);
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

    fn qa_entry(question: &str, answer: &str) -> crate::qa::QaEntry {
        crate::qa::QaEntry {
            question: question.into(),
            answer: answer.into(),
            source_title: "Programming Languages".into(),
            source_url: "/skills/programming-languages/".into(),
            source_section: None,
            tags: vec![],
            confidence: 0.9,
        }
    }

    fn qa_index(entries: Vec<crate::qa::QaEntry>) -> SearchIndex {
        let corpus = synthetic_corpus(4, 4, 5);
        let mut b = IndexBuilder::new();
        b.add_chunks(corpus.metadata.clone(), corpus.texts.clone(), vec![0; 4])
            .unwrap();
        b.add_qa(entries);
        b.finish().unwrap()
    }

    #[test]
    fn rank_qa_prefers_lexical_agreement_over_a_slightly_higher_cosine() {
        // The motivating case: synthesis wrote the exact answer, but the
        // dense score alone put an unrelated "years of experience" entry first.
        let index = qa_index(vec![
            qa_entry(
                "How many years of experience does Jason Grey have in AI/ML?",
                "Over ten years.",
            ),
            qa_entry("How long has Jason Grey been coding?", "Nearly 40 years."),
            qa_entry("Where does Jason Grey live?", "Minnesota."),
        ]);
        let query = "how long has jason been programming?";
        assert_eq!(
            qa_overlap_terms(query),
            vec!["long", "jason", "programming"]
        );
        let hits = rank_qa(&index, query, &[(0, 0.66), (1, 0.62), (2, 0.40)], 3);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].id, 1, "{:?}", hits);
        assert_eq!(hits[1].id, 0);
        let best = &hits[0];
        assert!((best.dense - 0.62).abs() < 1e-6);
        assert!((best.overlap - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(best.bm25_rank, Some(1));
        // 0.6·0.62 + 0.4·(0.5·0.667 + 0.5·1.0)
        let expected =
            QA_DENSE_WEIGHT * best.dense + QA_LEXICAL_WEIGHT * (0.5 * best.overlap + 0.5);
        assert!((best.score - expected).abs() < 1e-9);
        assert!((best.score - 0.7053).abs() < 1e-3);
        assert!(best.confident);
        assert!(!hits[1].confident, "{:?}", hits[1]);
        assert!(hits[1].overlap < QA_CONFIDENT_OVERLAP);
        assert!(hits[1].score < best.score);
        // Serialised shape carries every component.
        let json = serde_json::to_string(best).unwrap();
        for key in [
            "\"score\"",
            "\"dense\"",
            "\"overlap\"",
            "\"bm25_rank\"",
            "\"confident\"",
        ] {
            assert!(json.contains(key), "{}", json);
        }
    }

    #[test]
    fn rank_qa_without_dense_is_lexical_only_and_never_confident() {
        let index = qa_index(vec![
            qa_entry("How long has Jason Grey been coding?", "Nearly 40 years."),
            qa_entry("Where does Jason Grey live?", "Minnesota."),
        ]);
        let hits = rank_qa(&index, "how long has jason been programming?", &[], 5);
        assert_eq!(hits[0].id, 0);
        assert!(hits.iter().all(|h| !h.confident && h.dense == 0.0));
        assert!(hits[0].score <= QA_LEXICAL_WEIGHT + 1e-12);
        // A very high cosine alone is enough for confidence.
        let hits = rank_qa(&index, "zzz qqq", &[(1, 0.95)], 5);
        assert_eq!(hits[0].id, 1);
        assert_eq!(hits[0].overlap, 0.0);
        assert!(hits[0].confident);
        // Out-of-range ids are ignored; k = 0 and empty qa give nothing.
        assert!(rank_qa(&index, "x", &[(99, 0.9)], 5).is_empty());
        assert!(rank_qa(&index, "x", &[(0, 0.9)], 0).is_empty());
        let (no_qa, _) = build_synthetic_index(8, 4, Quant::F32, false);
        assert!(rank_qa(&no_qa, "topic1", &[(0, 0.9)], 3).is_empty());
        // Ties break by id; the list is capped at k.
        let hits = rank_qa(&index, "??", &[(1, 0.5), (0, 0.5)], 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 0);
        assert_eq!(qa_fetch_k(3), 20);
        assert_eq!(qa_fetch_k(8), 32);
    }

    #[test]
    fn qa_overlap_terms_drops_auxiliaries_unless_nothing_remains() {
        assert_eq!(
            qa_overlap_terms("Has he been there?"),
            vec!["has", "he", "been", "there"]
        );
        assert_eq!(
            qa_overlap_terms("What has Jason been building?"),
            vec!["jason", "building"]
        );
    }

    #[test]
    fn weights_parse_validates_three_numbers() {
        let w = Weights::parse("1,0.8, 0.6").unwrap();
        assert_eq!(
            w,
            Weights {
                dense: 1.0,
                sparse: 0.8,
                bm25: 0.6
            }
        );
        assert!(Weights::parse("1,2").is_err());
        assert!(Weights::parse("1,2,3,4").is_err());
        assert!(Weights::parse("a,1,1").is_err());
        assert!(Weights::parse("-1,1,1").is_err());
        assert!(Weights::parse("0,0,0").is_err());
        assert!(Weights::parse("nan,1,1").is_err());
    }

    #[test]
    fn fetch_k_and_rrf_k_overrides_change_the_fusion() {
        let (index, corpus) = index_2k();
        let base = query_for(&corpus, "topic3 facts", 3, Mode::Hybrid, 8);
        let r = retrieve(&index, &base).unwrap();
        let mut narrow = base.clone();
        narrow.fetch_k = Some(5);
        let n = retrieve(&index, &narrow).unwrap();
        assert!(n.ranked.len() <= 15 && n.ranked.len() < r.ranked.len());
        assert!(n.ranked.iter().all(|c| {
            c.dense_rank.is_none_or(|x| x <= 5)
                && c.sparse_rank.is_none_or(|x| x <= 5)
                && c.bm25_rank.is_none_or(|x| x <= 5)
        }));
        let mut sharp = base.clone();
        sharp.rrf_k = Some(0.0);
        let s = retrieve(&index, &sharp).unwrap();
        let top = s.ranked.iter().find(|c| c.chunk == 3).unwrap();
        assert!((top.score - (1.0 + 1.0 + 0.8)).abs() < 1e-9 || top.score >= 1.0);
        // Zero fetch_k and a non-finite rrf_k fall back to the defaults.
        let mut bad = base.clone();
        bad.fetch_k = Some(0);
        bad.rrf_k = Some(f64::NAN);
        assert_eq!(retrieve(&index, &bad).unwrap().ranked, r.ranked);
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
            dense: Some((1, vec![0.0, 0.0, 0.0, 1.0])),
            mode: Mode::Dense,
            top_k: 3,
            ..Query::default()
        };
        let r = retrieve(&index, &q).unwrap();
        assert_eq!(r.ranked.len(), 30);
        let q = Query {
            dense: Some((0, vec![0.0, 0.0, 0.0, 1.0])),
            mode: Mode::Dense,
            top_k: 3,
            ..Query::default()
        };
        assert!(retrieve(&index, &q).is_err());
    }
}
