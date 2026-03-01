// SPDX-License-Identifier: GPL-3.0-only

//! WASM bindings: expose search engine to JavaScript via wasm-bindgen.

use std::cell::RefCell;
use std::collections::HashMap;

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
    let mut best_per_url: HashMap<&str, (usize, f64)> = HashMap::new();

    for (chunk_idx, score) in &scored {
        let meta = &index.metadata[*chunk_idx];
        let entry = best_per_url.entry(meta.url.as_str()).or_insert((*chunk_idx, *score));
        if *score > entry.1 {
            *entry = (*chunk_idx, *score);
        }
    }

    let mut deduped: Vec<(usize, f64)> = best_per_url.into_values().collect();
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
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Find a word boundary near max_chars
    let truncated = &text[..max_chars];
    match truncated.rfind(' ') {
        Some(pos) => format!("{}…", &text[..pos]),
        None => format!("{}…", truncated),
    }
}
