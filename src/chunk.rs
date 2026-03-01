// SPDX-License-Identifier: GPL-3.0-only

//! Content chunking: split markdown/HTML into embeddable segments.

use serde::{Deserialize, Serialize};

/// Metadata extracted from a document's frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub date: Option<String>,
}

/// A parsed document with frontmatter metadata and body content.
#[derive(Debug, Clone)]
pub struct Document {
    pub meta: DocumentMeta,
    pub body: String,
    pub source_path: String,
}

/// Metadata attached to each chunk, linking it back to its source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub title: String,
    pub url: String,
    pub section: Option<String>,
    pub chunk_index: usize,
}

/// An embeddable text chunk with its metadata.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub meta: ChunkMeta,
}
