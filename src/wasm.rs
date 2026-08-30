// SPDX-License-Identifier: GPL-3.0-only

//! WASM bindings: the JS surface of the retriever.
//!
//! Every entry point returns `Result<_, JsValue>`; a panic hook forwards any
//! panic message to `console.error`. Results are JSON strings, so the surface
//! needs no object-mapping layer.
//!
//! ```text
//! capabilities() -> JSON {dense_wasm, sparse, version}         // what this build can do
//! manifest(index_bytes) -> JSON Manifest                       // header only
//! init_index(index_bytes)                                      // parse + validate
//! init_dense_wasm(lane_id, config, tokenizer, weights)         // bert lanes only; `dense-wasm` builds only
//! init_sparse_tokenizer(tokenizer_bytes)                       // enables the sparse arm; sha256 must match manifest.sparse.vocab_hash
//! attach_sidecar(bytes) -> JSON {lane, scopes}                 // loads an index.<lane>.ed sidecar's dense sections
//! search(query, top_k, mode, dense_lane_id|null, dense_query_vec|null)
//!     -> JSON {results:[PageResult], arms:{dense,sparse,bm25}, degraded:[string], mode, dense_lane}
//! page(url)   -> JSON {title, url, date, chunks:[{id, section, granularity, text}]}   // finest granularity only, document order
//! chunk(id)   -> JSON {id, title, url, section, date, text}
//! qa_lookup(query, dense_lane_id|null, dense_query_vec|null, k)
//!     -> JSON [{id, question, answer, source_title, source_url, source_section,
//!               score, dense, overlap, bm25_rank, confident}]
//!     // score = 0.6·dense + 0.4·lexical, see crate::search::rank_qa; without a
//!     // query vector the ranking is lexical only (never confident)
//! ```
//!
//! All ranking logic lives in [`crate::search`] so it is tested natively.
//!
//! Two builds share this file: the full one (`dense-wasm` feature, the
//! default) embeds queries with candle, the lite one (`--no-default-features`)
//! has no model code at all and relies on the worker to supply query vectors
//! (or to skip the dense arm). `capabilities()` tells the worker which one it
//! loaded.

use std::cell::RefCell;
use std::fmt::Display;
use std::sync::Once;

#[cfg(feature = "dense-wasm")]
use anyhow::Context;
use anyhow::{Result, anyhow};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[cfg(feature = "dense-wasm")]
use crate::embed::{DenseEncoder, bert_from_bytes};
use crate::index::{SearchIndex, query_vector_problem};
#[cfg(feature = "dense-wasm")]
use crate::manifest::{DenseSpec, Family, RuntimeSpec, TextKind};
use crate::search as rank;
use crate::search::{Mode, PageResult, Query, Weights};
use crate::sparse::{
    DEFAULT_MAX_SEQ_LEN, WordPiece, sparse_tokenizer_from_bytes, tokenizer_json_sha256,
};

#[cfg(feature = "dense-wasm")]
struct DenseRuntime {
    lane: usize,
    spec: DenseSpec,
    embedder: Box<dyn DenseEncoder>,
}

struct Engine {
    index: SearchIndex,
    #[cfg(feature = "dense-wasm")]
    dense: Option<DenseRuntime>,
    sparse_tokenizer: Option<WordPiece>,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

static PANIC_HOOK: Once = Once::new();

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&JsValue::from_str(&format!("eddie wasm panic: {}", info)));
        }));
    });
}

fn js_err(context: &str, e: impl Display) -> JsValue {
    JsValue::from_str(&format!("{}: {}", context, e))
}

fn with_engine<T>(f: impl FnOnce(&Engine) -> Result<T, JsValue>) -> Result<T, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("index not initialised: call init_index first"))?;
        f(engine)
    })
}

fn with_engine_mut<T>(f: impl FnOnce(&mut Engine) -> Result<T, JsValue>) -> Result<T, JsValue> {
    ENGINE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let engine = borrow
            .as_mut()
            .ok_or_else(|| JsValue::from_str("index not initialised: call init_index first"))?;
        f(engine)
    })
}

fn to_json<T: Serialize>(value: &T) -> Result<String, JsValue> {
    serde_json::to_string(value).map_err(|e| js_err("serializing result", e))
}

/// Embed one query with the lane's WASM embedder; the encoder applies the
/// lane's query prefix and truncation itself. This is the only place the
/// WASM module runs a model.
#[cfg(feature = "dense-wasm")]
fn embed_query(rt: &DenseRuntime, text: &str) -> Result<Vec<f32>> {
    let mut vecs = rt.embedder.embed(&[text], TextKind::Query)?;
    vecs.pop().context("embedder returned no vector")
}

#[derive(Serialize)]
struct Capabilities {
    /// `init_dense_wasm` exists: this build can embed queries on the CPU.
    dense_wasm: bool,
    /// `init_sparse_tokenizer` exists (always; the WordPiece tokenizer is
    /// built in).
    sparse: bool,
    /// Crate version the module was built from.
    version: &'static str,
}

/// What this WASM build can do, so the worker can branch between the lite
/// module (`{dense_wasm: false}`; load `eddie-dense.wasm` for a CPU lane)
/// and the full one.
#[wasm_bindgen]
pub fn capabilities() -> String {
    install_panic_hook();
    serde_json::to_string(&Capabilities {
        dense_wasm: cfg!(feature = "dense-wasm"),
        sparse: true,
        version: env!("CARGO_PKG_VERSION"),
    })
    .unwrap_or_else(|_| "{}".to_string())
}

/// Read the uncompressed manifest at the head of an `.ed` file.
#[wasm_bindgen]
pub fn manifest(index_bytes: &[u8]) -> Result<String, JsValue> {
    install_panic_hook();
    let m = SearchIndex::manifest_from_bytes(index_bytes)
        .map_err(|e| js_err("manifest parse failed", e))?;
    to_json(&m)
}

/// Parse and validate the index. Replaces any previously loaded index and
/// forgets its dense embedder / sparse tokenizer.
#[wasm_bindgen]
pub fn init_index(index_bytes: &[u8]) -> Result<(), JsValue> {
    install_panic_hook();
    let index = SearchIndex::from_bytes(index_bytes).map_err(|e| js_err("index load failed", e))?;
    ENGINE.with(|cell| {
        *cell.borrow_mut() = Some(Engine {
            index,
            #[cfg(feature = "dense-wasm")]
            dense: None,
            sparse_tokenizer: None,
        });
    });
    Ok(())
}

/// Load the BERT-family model for a `wasm-candle` lane of the loaded index.
/// Absent from the lite build (see [`capabilities`]).
#[cfg(feature = "dense-wasm")]
#[wasm_bindgen]
pub fn init_dense_wasm(
    lane_id: &str,
    config: &[u8],
    tokenizer: &[u8],
    weights: Vec<u8>,
) -> Result<(), JsValue> {
    install_panic_hook();
    with_engine_mut(|engine| {
        let spec = engine
            .index
            .manifest
            .dense_lane(lane_id)
            .ok_or_else(|| {
                js_err(
                    "init_dense_wasm",
                    format!("lane {:?} is not in the manifest", lane_id),
                )
            })?
            .clone();
        if !matches!(spec.runtime, RuntimeSpec::WasmCandle { .. }) {
            return Err(js_err(
                "init_dense_wasm",
                format!("lane {:?} is not a wasm-candle lane", lane_id),
            ));
        }
        if spec.family != Family::Bert {
            return Err(js_err(
                "init_dense_wasm",
                format!(
                    "lane {:?} family {:?} is not supported in WASM",
                    lane_id, spec.family
                ),
            ));
        }
        let lane = engine.index.dense_lane(lane_id).ok_or_else(|| {
            js_err(
                "init_dense_wasm",
                format!("index has no dense/chunks/{} section", lane_id),
            )
        })?;
        let embedder: Box<dyn DenseEncoder> = Box::new(
            bert_from_bytes(spec.clone(), config, tokenizer, weights)
                .map_err(|e| js_err("embedder init failed", e))?,
        );
        if embedder.dim() != spec.dim {
            return Err(js_err(
                "init_dense_wasm",
                format!(
                    "model produces {}-d vectors but lane {:?} stores {}-d",
                    embedder.dim(),
                    lane_id,
                    spec.dim
                ),
            ));
        }
        engine.dense = Some(DenseRuntime {
            lane,
            spec,
            embedder,
        });
        Ok(())
    })
}

/// Attach a sidecar file (`manifest.sidecars[].file`, fetched relative to
/// the index URL): its `dense/<scope>/<lane>` sections join the loaded
/// index after the identity (`index_id`), chunk count and lane shape are
/// checked. Until a lane's sidecar is attached, searches that ask for it
/// report the dense arm as skipped instead of failing.
#[wasm_bindgen]
pub fn attach_sidecar(bytes: &[u8]) -> Result<String, JsValue> {
    install_panic_hook();
    with_engine_mut(|engine| {
        let attached = engine
            .index
            .attach_sidecar(bytes)
            .map_err(|e| js_err("attach_sidecar", format!("{:#}", e)))?;
        to_json(&attached)
    })
}

/// Load the WordPiece tokenizer that enables the learned-sparse arm. The
/// bytes must be the `tokenizer.json` the index was built with: their
/// SHA-256 is checked against `manifest.sparse.vocab_hash`, because a
/// different vocabulary yields different token ids and a sparse arm that
/// scores noise while still looking healthy.
#[wasm_bindgen]
pub fn init_sparse_tokenizer(tokenizer_bytes: &[u8]) -> Result<(), JsValue> {
    install_panic_hook();
    with_engine_mut(|engine| {
        let spec = match (&engine.index.sparse, &engine.index.manifest.sparse) {
            (Some(_), Some(spec)) => spec,
            _ => {
                return Err(JsValue::from_str(
                    "init_sparse_tokenizer: index has no sparse arm",
                ));
            }
        };
        let actual = tokenizer_json_sha256(tokenizer_bytes);
        if !actual.eq_ignore_ascii_case(&spec.vocab_hash) {
            return Err(js_err(
                "init_sparse_tokenizer",
                format!(
                    "tokenizer.json sha256 {} does not match the manifest vocab_hash {} (expected {} at revision {})",
                    actual,
                    spec.vocab_hash,
                    spec.tokenizer,
                    spec.revision.as_deref().unwrap_or("main")
                ),
            ));
        }
        let tokenizer = sparse_tokenizer_from_bytes(tokenizer_bytes, DEFAULT_MAX_SEQ_LEN)
            .map_err(|e| js_err("sparse tokenizer load failed", e))?;
        engine.sparse_tokenizer = Some(tokenizer);
        Ok(())
    })
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<PageResult>,
    arms: rank::Arms,
    degraded: Vec<String>,
    mode: Mode,
    dense_lane: Option<String>,
}

/// Resolve the dense query vector: a caller-supplied vector (WebGPU lane) or
/// one embedding with the WASM lane. Returns the lane index, the lane id, the
/// vector, or a reason the dense arm is skipped.
///
/// Only caller misuse is an error: a vector without a lane id, an unknown
/// lane, or a vector of the wrong dimension. A vector that cannot rank
/// anything (NaN, all zeros) or an embedder failure degrades the dense arm
/// so BM25 and sparse still answer.
fn resolve_dense(
    engine: &Engine,
    lane_id: Option<&str>,
    vec: Option<Vec<f32>>,
    query: &str,
    want: bool,
) -> Result<(Option<(usize, String, Vec<f32>)>, Option<String>), JsValue> {
    if !want {
        return Ok((None, None));
    }
    if let Some(v) = vec {
        let id = lane_id.ok_or_else(|| {
            JsValue::from_str("dense_lane_id is required when dense_query_vec is given")
        })?;
        let Some(lane) = engine.index.dense_lane(id) else {
            // A lane the manifest lists but whose chunk vectors live in a
            // sidecar that is not attached yet degrades; a lane the manifest
            // never heard of is caller misuse.
            return match engine
                .index
                .manifest
                .sidecar_for(crate::index::SCOPE_CHUNKS, id)
            {
                Some(side) => Ok((
                    None,
                    Some(format!(
                        "dense: lane {:?} is not attached (sidecar {} not loaded)",
                        id, side.file
                    )),
                )),
                None => Err(js_err("search", format!("unknown dense lane {:?}", id))),
            };
        };
        let dim = engine.index.dense[lane].dim;
        if v.len() != dim {
            return Err(js_err(
                "search",
                format!(
                    "dense_query_vec has {} values but lane {:?} stores {}-d vectors",
                    v.len(),
                    id,
                    dim
                ),
            ));
        }
        if let Some(problem) = query_vector_problem(&v) {
            return Ok((
                None,
                Some(format!("dense: query vector for lane {:?} {}", id, problem)),
            ));
        }
        return Ok((Some((lane, id.to_string(), v)), None));
    }
    #[cfg(feature = "dense-wasm")]
    {
        match &engine.dense {
            Some(rt) if lane_id.is_none() || lane_id == Some(rt.spec.id.as_str()) => {
                match embed_query(rt, query) {
                    Ok(v) => Ok((Some((rt.lane, rt.spec.id.clone(), v)), None)),
                    Err(e) => Ok((
                        None,
                        Some(format!(
                            "dense: embedding failed for lane {:?}: {:#}",
                            rt.spec.id, e
                        )),
                    )),
                }
            }
            Some(rt) => Ok((
                None,
                Some(format!(
                    "dense: lane {:?} requested but only {:?} is loaded in WASM",
                    lane_id.unwrap_or(""),
                    rt.spec.id
                )),
            )),
            None => Ok((None, None)),
        }
    }
    #[cfg(not(feature = "dense-wasm"))]
    {
        // The lite build never embeds: without a caller-supplied vector the
        // dense arm is skipped and retrieve() reports it.
        let _ = (engine, query, lane_id);
        Ok((None, None))
    }
}

fn sparse_terms(
    engine: &Engine,
    query: &str,
) -> Result<Option<Vec<crate::manifest::SparseTerm>>, JsValue> {
    match (&engine.index.sparse, &engine.sparse_tokenizer) {
        (Some(sparse), Some(tokenizer)) => {
            let terms = rank::sparse_query_terms_local(tokenizer, &|id| sparse.idf_of(id), query)
                .map_err(|e| js_err("sparse query terms", e))?;
            Ok(Some(terms))
        }
        _ => Ok(None),
    }
}

/// Search the loaded index.
///
/// `mode`: `hybrid` (default), `dense`, `sparse`, or `keyword`.
/// `dense_lane_id` + `dense_query_vec`: a query vector produced outside WASM
/// (transformers.js) for the named lane. With both `null`, the WASM lane
/// loaded by `init_dense_wasm` embeds the query (exactly once); if none is
/// loaded the dense arm is skipped and reported in `degraded`.
#[wasm_bindgen]
pub fn search(
    query: &str,
    top_k: usize,
    mode: &str,
    dense_lane_id: Option<String>,
    dense_query_vec: Option<Vec<f32>>,
) -> Result<String, JsValue> {
    install_panic_hook();
    let mode =
        Mode::parse(mode).ok_or_else(|| js_err("search", format!("unknown mode {:?}", mode)))?;
    if top_k == 0 {
        return Err(JsValue::from_str("search: top_k must be > 0"));
    }
    with_engine(|engine| {
        let want_dense = matches!(mode, Mode::Hybrid | Mode::Dense);
        let (dense, mut degraded) = resolve_dense(
            engine,
            dense_lane_id.as_deref(),
            dense_query_vec,
            query,
            want_dense,
        )
        .map(|(d, reason)| (d, reason.into_iter().collect::<Vec<_>>()))?;
        let sparse = if matches!(mode, Mode::Hybrid | Mode::Sparse) {
            sparse_terms(engine, query)?
        } else {
            None
        };
        let dense_lane = dense.as_ref().map(|(_, id, _)| id.clone());
        let q = Query {
            text: query,
            dense: dense.map(|(lane, _, v)| (lane, v)),
            sparse,
            mode,
            top_k,
            weights: Weights::for_index(&engine.index),
            ..Query::default()
        };
        let retrieval =
            rank::retrieve(&engine.index, &q).map_err(|e| js_err("search failed", e))?;
        let terms = rank::query_terms(query);
        let results = rank::group_pages(&engine.index, &retrieval.ranked, &terms, top_k);
        // When this module already explained why the dense arm is missing,
        // retrieve()'s generic "no query vector" note describes the same
        // cause; keep one entry per cause.
        let dense_explained = !degraded.is_empty();
        degraded.extend(
            retrieval
                .degraded
                .into_iter()
                .filter(|note| !(dense_explained && note.starts_with("dense:"))),
        );
        to_json(&SearchResponse {
            results,
            arms: retrieval.arms,
            degraded,
            mode,
            dense_lane,
        })
    })
}

#[derive(Serialize)]
struct PageChunk<'a> {
    id: usize,
    section: Option<&'a str>,
    granularity: Option<&'a str>,
    text: &'a str,
}

#[derive(Serialize)]
struct PageView<'a> {
    title: &'a str,
    url: &'a str,
    date: Option<&'a str>,
    chunks: Vec<PageChunk<'a>>,
}

/// The text of a page (by URL) in document order: its chunks at the finest
/// granularity present (`fine` when the index has it), so coarse and summary
/// chunks never duplicate the same paragraphs.
#[wasm_bindgen]
pub fn page(url: &str) -> Result<String, JsValue> {
    install_panic_hook();
    with_engine(|engine| {
        let ids = engine.index.page_chunks_finest(url);
        let first = ids
            .first()
            .map(|&i| &engine.index.metadata[i])
            .ok_or_else(|| js_err("page", format!("no page with url {:?}", url)))?;
        let chunks = ids
            .iter()
            .map(|&i| {
                let meta = &engine.index.metadata[i];
                PageChunk {
                    id: i,
                    section: meta.section.as_deref(),
                    granularity: meta.granularity.as_deref(),
                    text: engine.index.texts.get(i).map(String::as_str).unwrap_or(""),
                }
            })
            .collect();
        to_json(&PageView {
            title: &first.title,
            url: &first.url,
            date: first.date.as_deref(),
            chunks,
        })
    })
}

#[derive(Serialize)]
struct ChunkView<'a> {
    id: usize,
    title: &'a str,
    url: &'a str,
    section: Option<&'a str>,
    date: Option<&'a str>,
    text: &'a str,
}

/// One chunk by id.
#[wasm_bindgen]
pub fn chunk(id: usize) -> Result<String, JsValue> {
    install_panic_hook();
    with_engine(|engine| {
        let meta = engine
            .index
            .metadata
            .get(id)
            .ok_or_else(|| js_err("chunk", format!("chunk {} is out of range", id)))?;
        to_json(&ChunkView {
            id,
            title: &meta.title,
            url: &meta.url,
            section: meta.section.as_deref(),
            date: meta.date.as_deref(),
            text: engine.index.texts.get(id).map(String::as_str).unwrap_or(""),
        })
    })
}

#[derive(Serialize)]
struct QaHit<'a> {
    id: usize,
    question: &'a str,
    answer: &'a str,
    source_title: &'a str,
    source_url: &'a str,
    source_section: Option<&'a str>,
    /// Fused score (see [`crate::search::rank_qa`]); `dense`, `overlap`,
    /// `bm25_rank` and `confident` are its components.
    score: f64,
    dense: f64,
    overlap: f64,
    bm25_rank: Option<usize>,
    confident: bool,
}

/// QA entries ranked by [`crate::search::rank_qa`]: the qa dense lane's
/// cosine fused with lexical overlap and a BM25 pass over the questions and
/// answers. Returns `[]` when the index has no qa section; without a query
/// vector (no runnable embedder) the ranking is lexical only and no hit is
/// `confident`.
#[wasm_bindgen]
pub fn qa_lookup(
    query: &str,
    dense_lane_id: Option<String>,
    dense_query_vec: Option<Vec<f32>>,
    k: usize,
) -> Result<String, JsValue> {
    install_panic_hook();
    with_engine(|engine| {
        if engine.index.qa.is_empty() || k == 0 {
            return Ok("[]".to_string());
        }
        let (dense, _) = resolve_dense(
            engine,
            dense_lane_id.as_deref(),
            dense_query_vec,
            query,
            true,
        )?;
        let dense_hits: Vec<(usize, f32)> = match dense {
            Some((_, lane_id, vec)) => match engine.index.qa_lane(&lane_id) {
                Some(lane) => lane
                    .top_k(&vec, rank::qa_fetch_k(k))
                    .map_err(|e| js_err("qa_lookup", anyhow!("{}", e)))?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let out: Vec<QaHit> = rank::rank_qa(&engine.index, query, &dense_hits, k)
            .into_iter()
            .map(|h| {
                let e = &engine.index.qa[h.id];
                QaHit {
                    id: h.id,
                    question: &e.question,
                    answer: &e.answer,
                    source_title: &e.source_title,
                    source_url: &e.source_url,
                    source_section: e.source_section.as_deref(),
                    score: h.score,
                    dense: h.dense,
                    overlap: h.overlap,
                    bm25_rank: h.bm25_rank,
                    confident: h.confident,
                }
            })
            .collect();
        to_json(&out)
    })
}
