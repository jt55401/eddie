// SPDX-License-Identifier: GPL-3.0-only

//! Serde records shared by the indexer and the runtime: chunk metadata and
//! the optional QA / claims entries stored in an index. Pure data, so the
//! lite WASM build carries them without the parsers, chunkers and
//! extractors (`chunk`, `qa`, `claims`) that produce them natively.

use serde::{Deserialize, Serialize};

/// Metadata attached to each chunk, linking it back to its source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub title: String,
    pub url: String,
    pub section: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub granularity: Option<String>,
    pub chunk_index: usize,
}

/// One question/answer pair of the `qa` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaEntry {
    pub question: String,
    pub answer: String,
    pub source_title: String,
    pub source_url: String,
    pub source_section: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
}

/// One extracted claim of the `claims` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimEntry {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub evidence: String,
    pub source_title: String,
    pub source_url: String,
    pub source_section: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
}
