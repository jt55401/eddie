// SPDX-License-Identifier: GPL-3.0-only

//! WASM bindings: expose search engine to JavaScript via wasm-bindgen.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::bm25::hybrid_rrf;
use crate::embed::Embedder;
use crate::index::SearchIndex;
use crate::search::search;

struct SearchEngine {
    embedder: Embedder,
    index: SearchIndex,
}

thread_local! {
    static ENGINE: RefCell<Option<SearchEngine>> = const { RefCell::new(None) };
}

#[wasm_bindgen]
pub fn extract_model_id(index_bytes: &[u8]) -> Result<String, JsValue> {
    SearchIndex::model_id_from_bytes(index_bytes)
        .map_err(|e| JsValue::from_str(&format!("model_id parse failed: {}", e)))
}

/// Initialize the search engine from raw model and index bytes.
///
/// Called once by the Web Worker after downloading all assets.
#[wasm_bindgen]
pub fn init_engine(
    config_bytes: &[u8],
    tokenizer_bytes: &[u8],
    weights_bytes: Vec<u8>,
    index_bytes: &[u8],
) -> Result<(), JsValue> {
    let embedder = Embedder::from_bytes(config_bytes, tokenizer_bytes, weights_bytes)
        .map_err(|e| JsValue::from_str(&format!("embedder init failed: {}", e)))?;

    let index = SearchIndex::from_bytes(index_bytes)
        .map_err(|e| JsValue::from_str(&format!("index load failed: {}", e)))?;

    ENGINE.with(|cell| {
        *cell.borrow_mut() = Some(SearchEngine { embedder, index });
    });

    Ok(())
}

#[derive(Serialize)]
struct WasmSearchResult {
    title: String,
    url: String,
    section: Option<String>,
    snippet: String,
    score: f64,
}

#[derive(Serialize)]
struct WasmQaHit {
    question: String,
    answer: String,
    source_url: String,
    score: f64,
}

#[derive(Serialize)]
struct WasmClaimHit {
    subject: String,
    predicate: String,
    object: String,
    evidence: String,
    source_url: String,
    score: f64,
}

#[derive(Serialize)]
struct WasmAnswer {
    text: String,
    citations: Vec<String>,
    lane: String,
}

#[derive(Serialize)]
struct WasmSearchBundle {
    results: Vec<WasmSearchResult>,
    answer: Option<WasmAnswer>,
}

const RECENCY_ALPHA: f64 = 0.18;
const RECENCY_HALFLIFE_YEARS: f64 = 4.0;

/// Search the index and return results as a JS array.
///
/// `mode`: "semantic", "keyword", or "hybrid" (default).
#[wasm_bindgen]
pub fn search_query(query: &str, top_k: usize, mode: &str) -> Result<JsValue, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("engine not initialized"))?;

        let results = match mode {
            "semantic" => search_semantic(engine, query, top_k),
            "keyword" => search_keyword(engine, query, top_k),
            _ => search_hybrid(engine, query, top_k),
        }
        .map_err(|e| JsValue::from_str(&format!("search failed: {}", e)))?;

        serde_wasm_bindgen::to_value(&results)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {}", e)))
    })
}

/// Search and optionally synthesize a grounded answer inside WASM.
#[wasm_bindgen]
pub fn search_with_answer(
    query: &str,
    top_k: usize,
    answer_top_k: usize,
    mode: &str,
    answer_mode: bool,
    qa_subject: &str,
) -> Result<JsValue, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("engine not initialized"))?;

        let results = match mode {
            "semantic" => search_semantic(engine, query, top_k),
            "keyword" => search_keyword(engine, query, top_k),
            _ => search_hybrid(engine, query, top_k),
        }
        .map_err(|e| JsValue::from_str(&format!("search failed: {}", e)))?;

        let answer = if answer_mode {
            let query_vec = engine
                .embedder
                .embed_batch(&[query])
                .map_err(|e| JsValue::from_str(&format!("embedding failed: {}", e)))?;

            let qa_hits = query_qa_hits_with_vec(engine, &query_vec[0], answer_top_k);
            let claim_hits = query_claim_hits_with_vec(engine, &query_vec[0], answer_top_k);
            synthesize_answer(query, qa_subject, &results, &qa_hits, &claim_hits)
        } else {
            None
        };

        let bundle = WasmSearchBundle { results, answer };
        serde_wasm_bindgen::to_value(&bundle)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {}", e)))
    })
}

/// Query the embedded QA section semantically.
#[wasm_bindgen]
pub fn query_qa(query: &str, top_k: usize) -> Result<JsValue, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("engine not initialized"))?;
        let q = engine
            .embedder
            .embed_batch(&[query])
            .map_err(|e| JsValue::from_str(&format!("embedding failed: {}", e)))?;
        let out = query_qa_hits_with_vec(engine, &q[0], top_k);

        serde_wasm_bindgen::to_value(&out)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {}", e)))
    })
}

/// Query the embedded claims section semantically.
#[wasm_bindgen]
pub fn query_claims(query: &str, top_k: usize) -> Result<JsValue, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("engine not initialized"))?;
        let q = engine
            .embedder
            .embed_batch(&[query])
            .map_err(|e| JsValue::from_str(&format!("embedding failed: {}", e)))?;
        let out = query_claim_hits_with_vec(engine, &q[0], top_k);

        serde_wasm_bindgen::to_value(&out)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {}", e)))
    })
}

fn search_semantic(
    engine: &SearchEngine,
    query: &str,
    top_k: usize,
) -> Result<Vec<WasmSearchResult>, anyhow::Error> {
    let query_vecs = engine.embedder.embed_batch(&[query])?;
    let results = search(&engine.index, &query_vecs[0], top_k);

    Ok(dedup_results(
        results
            .into_iter()
            .map(|r| (r.chunk_index, r.score as f64))
            .collect(),
        &engine.index,
        top_k,
    ))
}

fn search_keyword(
    engine: &SearchEngine,
    query: &str,
    top_k: usize,
) -> Result<Vec<WasmSearchResult>, anyhow::Error> {
    let results = engine.index.bm25.search(query, top_k);

    Ok(dedup_results(results, &engine.index, top_k))
}

fn search_hybrid(
    engine: &SearchEngine,
    query: &str,
    top_k: usize,
) -> Result<Vec<WasmSearchResult>, anyhow::Error> {
    let fetch_k = top_k * 3;

    let query_vecs = engine.embedder.embed_batch(&[query])?;
    let semantic_results = search(&engine.index, &query_vecs[0], fetch_k);
    let bm25_results = engine.index.bm25.search(query, fetch_k);

    let semantic_pairs: Vec<(usize, f32)> = semantic_results
        .iter()
        .map(|r| (r.chunk_index, r.score))
        .collect();

    let hybrid = hybrid_rrf(&semantic_pairs, &bm25_results, fetch_k);

    Ok(dedup_results(hybrid, &engine.index, top_k))
}

/// Deduplicate results by URL, keeping the best-scoring chunk per page,
/// and build WasmSearchResult with truncated snippets.
fn dedup_results(
    scored: Vec<(usize, f64)>,
    index: &SearchIndex,
    top_k: usize,
) -> Vec<WasmSearchResult> {
    let mut best_per_url: HashMap<&str, (usize, f64, HashMap<String, f64>)> = HashMap::new();

    for (chunk_idx, score) in &scored {
        let meta = &index.metadata[*chunk_idx];
        let recency = recency_boost(meta.date.as_deref());
        let adjusted = *score * recency;
        let granularity = meta
            .granularity
            .clone()
            .unwrap_or_else(|| "fine".to_string());
        let entry =
            best_per_url
                .entry(meta.url.as_str())
                .or_insert((*chunk_idx, adjusted, HashMap::new()));
        let gran = entry.2.entry(granularity).or_insert(adjusted);
        if adjusted > *gran {
            *gran = adjusted;
        }
        if adjusted > entry.1 {
            entry.0 = *chunk_idx;
            entry.1 = adjusted;
        }
    }

    let mut deduped: Vec<(usize, f64)> = best_per_url
        .into_values()
        .map(|(idx, best, gran_scores)| (idx, best + granularity_fusion_bonus(&gran_scores)))
        .collect();
    deduped.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    deduped.truncate(top_k);

    deduped
        .into_iter()
        .map(|(chunk_idx, score)| {
            let meta = &index.metadata[chunk_idx];
            let snippet = if chunk_idx < index.texts.len() {
                truncate_snippet(&index.texts[chunk_idx], 150)
            } else {
                String::new()
            };

            WasmSearchResult {
                title: meta.title.clone(),
                url: meta.url.clone(),
                section: meta.section.clone(),
                snippet,
                score,
            }
        })
        .collect()
}

fn truncate_snippet(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    // Build a safe UTF-8 boundary at max_chars.
    let byte_end = text
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    let truncated = &text[..byte_end];

    // Prefer the last whitespace break for cleaner snippets.
    match truncated.rfind(char::is_whitespace) {
        Some(pos) if pos > 0 => format!("{}…", truncated[..pos].trim_end()),
        _ => format!("{}…", truncated.trim_end()),
    }
}

fn query_qa_hits_with_vec(
    engine: &SearchEngine,
    query_vec: &[f32],
    top_k: usize,
) -> Vec<WasmQaHit> {
    if engine.index.qa_entries.is_empty()
        || engine.index.qa_embeddings.is_empty()
        || engine.index.qa_dim == 0
    {
        return Vec::new();
    }

    let hits = semantic_top_n(
        &engine.index.qa_embeddings,
        engine.index.qa_dim,
        query_vec,
        top_k,
    );
    hits.into_iter()
        .filter_map(|(idx, score)| engine.index.qa_entries.get(idx).map(|entry| (entry, score)))
        .map(|(entry, score)| WasmQaHit {
            question: entry.question.clone(),
            answer: entry.answer.clone(),
            source_url: entry.source_url.clone(),
            score: score as f64,
        })
        .collect()
}

fn query_claim_hits_with_vec(
    engine: &SearchEngine,
    query_vec: &[f32],
    top_k: usize,
) -> Vec<WasmClaimHit> {
    if engine.index.claims.is_empty()
        || engine.index.claim_embeddings.is_empty()
        || engine.index.claim_dim == 0
    {
        return Vec::new();
    }

    let hits = semantic_top_n(
        &engine.index.claim_embeddings,
        engine.index.claim_dim,
        query_vec,
        top_k,
    );
    let recency_by_url = build_url_recency_map(&engine.index);
    let mut out: Vec<WasmClaimHit> = hits
        .into_iter()
        .filter_map(|(idx, score)| engine.index.claims.get(idx).map(|claim| (claim, score)))
        .map(|(claim, score)| WasmClaimHit {
            subject: claim.subject.clone(),
            predicate: claim.predicate.clone(),
            object: claim.object.clone(),
            evidence: claim.evidence.clone(),
            source_url: claim.source_url.clone(),
            score: (score as f64)
                * recency_by_url
                    .get(claim.source_url.as_str())
                    .copied()
                    .unwrap_or(1.0),
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(top_k);
    out
}

struct EvidenceItem {
    lane: String,
    text: String,
    url: String,
    raw_score: f64,
    predicate: Option<String>,
}

#[derive(Clone)]
struct ScoredEvidence {
    lane: String,
    text: String,
    url: String,
    score: f64,
    matched: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryIntent {
    Knowledge,
    WorkHistory,
    Duration,
    Generic,
}

struct QueryAnalysis {
    terms: Vec<String>,
    intent: QueryIntent,
    yes_no: bool,
}

fn synthesize_answer(
    query: &str,
    qa_subject: &str,
    search_hits: &[WasmSearchResult],
    qa_hits: &[WasmQaHit],
    claim_hits: &[WasmClaimHit],
) -> Option<WasmAnswer> {
    let evidence = collect_evidence(search_hits, qa_hits, claim_hits);
    if evidence.is_empty() {
        return None;
    }

    let analysis = analyze_query(query, qa_subject);
    let mut ranked: Vec<ScoredEvidence> = evidence
        .into_iter()
        .map(|item| score_evidence(item, &analysis))
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let picked = select_answer_evidence(&ranked, &analysis);
    if picked.is_empty() {
        return Some(WasmAnswer {
            text: "I couldn't find strong evidence for that in the current index.".to_string(),
            citations: Vec::new(),
            lane: "search".to_string(),
        });
    }

    let mut answer_parts = Vec::new();
    let mut citations = Vec::new();
    for item in &picked {
        let sentence = normalize_answer_sentence(&item.text);
        if !sentence.is_empty() {
            answer_parts.push(sentence);
        }
        if !item.url.is_empty() && !citations.iter().any(|u| u == &item.url) {
            citations.push(item.url.clone());
        }
    }

    let text = answer_parts.join(" ");
    if text.is_empty() {
        return None;
    }

    Some(WasmAnswer {
        text,
        citations: citations.into_iter().take(3).collect(),
        lane: picked[0].lane.clone(),
    })
}

fn collect_evidence(
    search_hits: &[WasmSearchResult],
    qa_hits: &[WasmQaHit],
    claim_hits: &[WasmClaimHit],
) -> Vec<EvidenceItem> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |lane: &str, text: &str, url: &str, raw_score: f64, predicate: Option<&str>| {
        let clean = normalize_ws(text);
        if clean.is_empty() {
            return;
        }
        let key = format!("{}|{}|{}", lane, url, clean.to_lowercase());
        if !seen.insert(key) {
            return;
        }
        out.push(EvidenceItem {
            lane: lane.to_string(),
            text: clean,
            url: url.to_string(),
            raw_score: if raw_score.is_finite() {
                raw_score
            } else {
                0.0
            },
            predicate: predicate.map(|p| p.to_string()),
        });
    };

    for hit in qa_hits.iter().take(10) {
        let text = if !hit.answer.trim().is_empty() {
            &hit.answer
        } else {
            &hit.question
        };
        push("qa", text, &hit.source_url, hit.score, None);
    }

    for hit in claim_hits.iter().take(12) {
        let sentence = claim_to_sentence(hit);
        push(
            "claims",
            &sentence,
            &hit.source_url,
            hit.score,
            Some(hit.predicate.as_str()),
        );
    }

    for hit in search_hits.iter().take(12) {
        let text = if !hit.snippet.trim().is_empty() {
            &hit.snippet
        } else {
            &hit.title
        };
        push("search", text, &hit.url, hit.score, None);
    }

    out
}

fn claim_to_sentence(hit: &WasmClaimHit) -> String {
    let predicate = hit.predicate.trim();
    let object = hit.object.trim();
    if predicate.is_empty() && object.is_empty() {
        return String::new();
    }

    if predicate == "worked_for" {
        return format!("Worked for {}.", object).trim().to_string();
    }
    if predicate == "has_skill" {
        return format!("Has skill {}.", object).trim().to_string();
    }
    if let Some(activity) = predicate.strip_prefix("years_") {
        let activity = activity.replace('_', " ");
        return format!("{} of {} experience.", object, activity)
            .trim()
            .to_string();
    }
    if let Some(activity) = predicate.strip_prefix("since_age_") {
        let activity = activity.replace('_', " ");
        return format!("Since age {} in {}.", object, activity)
            .trim()
            .to_string();
    }

    format!("{} {}.", predicate.replace('_', " "), object)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn analyze_query(query: &str, qa_subject: &str) -> QueryAnalysis {
    let lower = normalize_query_for_parse(query);
    QueryAnalysis {
        terms: query_tokens(query, qa_subject),
        intent: detect_query_intent(&lower),
        yes_no: is_yes_no_question(&lower),
    }
}

fn query_tokens(query: &str, qa_subject: &str) -> Vec<String> {
    let stop_words: HashSet<&'static str> = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "do", "does", "did",
        "has", "have", "had", "in", "on", "at", "of", "to", "for", "with", "and", "or", "by",
        "from", "as", "it", "this", "that", "who", "what", "when", "where", "why", "how", "many",
        "long", "since", "years", "know",
    ]
    .into_iter()
    .collect();

    let mut subject_terms: HashSet<String> = HashSet::new();
    for token in qa_subject.to_lowercase().split_whitespace() {
        if !token.is_empty() {
            subject_terms.insert(token.to_string());
        }
    }
    for token in infer_subject_terms(query) {
        if !token.is_empty() {
            subject_terms.insert(token);
        }
    }

    let mut normalized = String::with_capacity(query.len());
    for ch in query.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '-' | '_' | ' ') {
            normalized.push(ch);
        } else {
            normalized.push(' ');
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for token in normalized.split_whitespace() {
        for variant in expand_query_term(token) {
            if variant.len() <= 1 {
                continue;
            }
            if stop_words.contains(variant.as_str()) {
                continue;
            }
            if subject_terms.contains(variant.as_str()) {
                continue;
            }
            if seen.insert(variant.clone()) {
                out.push(variant);
            }
        }
    }

    out
}

fn score_evidence(item: EvidenceItem, query: &QueryAnalysis) -> ScoredEvidence {
    let hay = item.text.to_lowercase();
    let mut matched = Vec::new();
    for term in &query.terms {
        if hay.contains(term) {
            matched.push(term.clone());
        }
    }

    let coverage = if query.terms.is_empty() {
        0.0
    } else {
        matched.len() as f64 / query.terms.len() as f64
    };
    let lane_weight = match item.lane.as_str() {
        "qa" => 0.95,
        "claims" => 0.9,
        _ => 0.8,
    };
    let raw = item.raw_score.clamp(0.0, 1.0);
    let intent_bonus = intent_alignment_bonus(
        query.intent,
        item.lane.as_str(),
        item.predicate.as_deref(),
        hay.as_str(),
    );
    let yes_no_bonus = if query.yes_no && query.intent == QueryIntent::Knowledge {
        match item.predicate.as_deref() {
            Some("has_skill") => 0.22,
            Some("worked_for") => -0.2,
            _ => 0.0,
        }
    } else {
        0.0
    };
    let score = lane_weight + coverage * 1.4 + raw * 0.25 + intent_bonus + yes_no_bonus;

    ScoredEvidence {
        lane: item.lane,
        text: item.text,
        url: item.url,
        score,
        matched,
    }
}

fn select_answer_evidence(ranked: &[ScoredEvidence], query: &QueryAnalysis) -> Vec<ScoredEvidence> {
    if ranked.is_empty() {
        return Vec::new();
    }
    if !query.terms.is_empty() && ranked.iter().all(|item| item.matched.is_empty()) {
        return Vec::new();
    }

    let min_score = match query.intent {
        QueryIntent::Knowledge => 1.1,
        QueryIntent::WorkHistory | QueryIntent::Duration => 1.0,
        QueryIntent::Generic => 0.95,
    };
    let mut picked = Vec::new();
    let mut covered: HashSet<&str> = HashSet::new();
    for item in ranked {
        if picked.len() >= 2 {
            break;
        }
        if item.score < min_score {
            continue;
        }
        if !query.terms.is_empty() && item.matched.is_empty() {
            continue;
        }

        let adds_coverage = item
            .matched
            .iter()
            .any(|term| !covered.contains(term.as_str()));
        if picked.is_empty() || adds_coverage {
            for term in &item.matched {
                covered.insert(term);
            }
            picked.push(item.clone());
        }
    }

    if !picked.is_empty() {
        return picked;
    }
    if query.terms.is_empty() && ranked[0].score >= min_score {
        return vec![ranked[0].clone()];
    }
    Vec::new()
}

fn normalize_query_for_parse(query: &str) -> String {
    let mut out = String::new();
    for ch in query.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '+') {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    normalize_ws(out.as_str())
}

fn detect_query_intent(lower_query: &str) -> QueryIntent {
    let padded = format!(" {} ", lower_query);
    if padded.contains(" worked for ")
        || padded.contains(" worked at ")
        || padded.contains(" employer ")
        || padded.contains(" employers ")
        || padded.contains(" companies ")
        || padded.contains(" company ")
    {
        return QueryIntent::WorkHistory;
    }
    if padded.contains("how many years")
        || padded.contains("how long")
        || padded.contains("since age")
        || padded.contains(" years has ")
        || padded.contains(" years of ")
    {
        return QueryIntent::Duration;
    }
    if padded.contains(" know ")
        || padded.contains(" knows ")
        || padded.contains(" familiar with ")
        || padded.contains(" proficient ")
        || padded.contains(" skill ")
        || padded.contains(" skills ")
        || padded.contains(" expertise ")
        || padded.contains(" experience with ")
        || padded.contains(" use ")
        || padded.contains(" uses ")
    {
        return QueryIntent::Knowledge;
    }
    QueryIntent::Generic
}

fn is_yes_no_question(lower_query: &str) -> bool {
    lower_query.starts_with("does ")
        || lower_query.starts_with("is ")
        || lower_query.starts_with("can ")
        || lower_query.starts_with("has ")
        || lower_query.starts_with("have ")
        || lower_query.starts_with("did ")
}

fn infer_subject_terms(query: &str) -> HashSet<String> {
    let q = normalize_query_for_parse(query);
    let mut terms = HashSet::new();
    let patterns: [(&str, &[&str]); 8] = [
        ("does ", &[" know ", " use ", " have ", " work ", " do "]),
        ("is ", &[" familiar with ", " skilled in ", " good at "]),
        ("who has ", &[" worked", " work "]),
        ("how many years has ", &[" been "]),
        ("how long has ", &[" been "]),
        ("since what age has ", &[" been "]),
        ("has ", &[" worked for ", " worked at "]),
        ("what has ", &[" worked on ", " done "]),
    ];

    for (prefix, suffixes) in patterns {
        if let Some(subject) = capture_between_any(&q, prefix, suffixes) {
            for token in subject.split_whitespace() {
                if token.len() > 1 {
                    terms.insert(token.to_string());
                }
            }
        }
    }
    terms
}

fn capture_between_any<'a>(input: &'a str, prefix: &str, suffixes: &[&str]) -> Option<&'a str> {
    let rest = input.strip_prefix(prefix)?;
    let mut best: Option<&str> = None;
    let mut best_pos = usize::MAX;
    for suffix in suffixes {
        if let Some(pos) = rest.find(suffix) {
            if pos < best_pos {
                best = Some(rest[..pos].trim());
                best_pos = pos;
            }
        }
    }
    best.filter(|s| !s.is_empty() && s.split_whitespace().count() <= 4)
}

fn expand_query_term(token: &str) -> Vec<String> {
    let mut out = Vec::new();
    let base = token.trim().to_string();
    if base.is_empty() {
        return out;
    }
    out.push(base.clone());

    if base.contains('-') || base.contains('/') || base.contains('_') {
        let compact = base.replace(['-', '/', '_'], "");
        if compact.len() > 1 {
            out.push(compact);
        }
        for part in base.split(['-', '/', '_']) {
            let p = part.trim();
            if p.len() > 1 {
                out.push(p.to_string());
            }
        }
    }

    out
}

fn intent_alignment_bonus(
    intent: QueryIntent,
    lane: &str,
    predicate: Option<&str>,
    text_lower: &str,
) -> f64 {
    let pred = predicate.unwrap_or_default();
    match intent {
        QueryIntent::Knowledge => {
            if pred == "has_skill" {
                return 0.95;
            }
            if pred == "worked_for" {
                return -1.1;
            }
            if pred.starts_with("years_") || pred.starts_with("since_age_") {
                return -0.45;
            }
            if text_lower.contains("worked for ") {
                return -0.8;
            }
            if lane == "search" {
                return 0.12;
            }
            if lane == "qa" {
                return 0.16;
            }
            0.0
        }
        QueryIntent::WorkHistory => {
            if pred == "worked_for" {
                return 0.95;
            }
            if pred == "has_skill" {
                return -0.7;
            }
            if pred.starts_with("years_") || pred.starts_with("since_age_") {
                return -0.3;
            }
            if text_lower.contains("worked for ") {
                return 0.45;
            }
            0.0
        }
        QueryIntent::Duration => {
            if pred.starts_with("years_") || pred.starts_with("since_age_") {
                return 0.95;
            }
            if pred == "worked_for" {
                return -0.8;
            }
            if pred == "has_skill" {
                return -0.4;
            }
            if text_lower.contains("years ") || text_lower.contains("since age ") {
                return 0.35;
            }
            0.0
        }
        QueryIntent::Generic => 0.0,
    }
}

fn normalize_answer_sentence(text: &str) -> String {
    let clean = normalize_ws(text);
    if clean.is_empty() {
        return String::new();
    }
    if clean.ends_with('.') || clean.ends_with('!') || clean.ends_with('?') || clean.ends_with('…')
    {
        return clean;
    }
    format!("{}.", clean)
}

fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn semantic_top_n(flat: &[f32], dim: usize, query: &[f32], top_k: usize) -> Vec<(usize, f32)> {
    if dim == 0 || query.len() != dim || flat.is_empty() {
        return Vec::new();
    }
    let rows = flat.len() / dim;
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let start = row * dim;
        let emb = &flat[start..start + dim];
        let score = emb
            .iter()
            .zip(query.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>();
        out.push((row, score));
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(top_k);
    out
}

fn build_url_recency_map(index: &SearchIndex) -> HashMap<&str, f64> {
    let mut out = HashMap::new();
    for meta in &index.metadata {
        let boost = recency_boost(meta.date.as_deref());
        out.entry(meta.url.as_str())
            .and_modify(|v| {
                if boost > *v {
                    *v = boost;
                }
            })
            .or_insert(boost);
    }
    out
}

fn granularity_fusion_bonus(granularity_scores: &HashMap<String, f64>) -> f64 {
    if granularity_scores.len() <= 1 {
        return 0.0;
    }
    let mut vals: Vec<f64> = granularity_scores.values().copied().collect();
    vals.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    // Reward agreement across chunk granularities/lane summaries.
    vals.into_iter().skip(1).sum::<f64>() * 0.12
}

fn recency_boost(date: Option<&str>) -> f64 {
    let Some((year, month)) = parse_year_month(date.unwrap_or_default()) else {
        return 1.0;
    };
    let current = current_year_estimate();
    let source = year as f64 + ((month as f64 - 0.5) / 12.0);
    let age_years = (current - source).max(0.0);
    let decay = 2f64.powf(-age_years / RECENCY_HALFLIFE_YEARS);
    1.0 + (RECENCY_ALPHA * decay)
}

fn parse_year_month(date: &str) -> Option<(i32, u32)> {
    let trimmed = date.trim();
    if trimmed.len() < 4 {
        return None;
    }
    let year = trimmed.get(0..4)?.parse::<i32>().ok()?;
    let month = trimmed
        .get(5..7)
        .and_then(|m| m.parse::<u32>().ok())
        .filter(|m| (1..=12).contains(m))
        .unwrap_or(6);
    Some((year, month))
}

fn current_year_estimate() -> f64 {
    // Browser-safe clock source for wasm32.
    let millis = js_sys::Date::now();
    1970.0 + ((millis / 1000.0) / (365.2425 * 24.0 * 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_terms_drop_subject_for_knowledge_question() {
        let terms = query_tokens("does jason know k-means?", "");
        assert!(terms.iter().any(|t| t == "k-means" || t == "kmeans"));
        assert!(!terms.iter().any(|t| t == "jason"));
    }

    #[test]
    fn knowledge_intent_penalizes_work_history_claims() {
        let query = analyze_query("does jason know k-means?", "");
        let score_work = score_evidence(
            EvidenceItem {
                lane: "claims".to_string(),
                text: "Worked for Common Crawl.".to_string(),
                url: "/about/".to_string(),
                raw_score: 0.9,
                predicate: Some("worked_for".to_string()),
            },
            &query,
        );
        let score_skill = score_evidence(
            EvidenceItem {
                lane: "claims".to_string(),
                text: "Has skill K-Means.".to_string(),
                url: "/skills/".to_string(),
                raw_score: 0.6,
                predicate: Some("has_skill".to_string()),
            },
            &query,
        );
        assert!(score_skill.score > score_work.score);
    }
}
