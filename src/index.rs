// SPDX-License-Identifier: GPL-3.0-only

//! Index format: serialize and deserialize the vector index + metadata.
//!
//! Binary format (v4):
//! - `SAGI` magic (4 bytes)
//! - version: u32 LE (4)
//! - dim: u32 LE (chunk embedding dimensionality)
//! - num_chunks: u32 LE
//! - model_id_len: u32 LE
//! - metadata_len: u32 LE
//! - model_id: UTF-8 bytes
//! - metadata: JSON-encoded `Vec<ChunkMeta>` bytes
//! - embeddings: `num_chunks * dim` f32 values (LE)
//! - bm25_index: length-prefixed JSON (u32 LE length + JSON bytes)
//! - text_count: u32 LE
//! - texts: for each text, u32 LE length + UTF-8 bytes
//! - section_count: u32 LE
//! - repeated section payloads:
//!   - section_name_len: u32 LE
//!   - section_name: UTF-8 bytes (`qa`, `claims`, ...)
//!   - section_item_count: u32 LE
//!   - section_json_len: u32 LE
//!   - section_json: JSON-encoded entries
//!   - section_dim: u32 LE
//!   - section_embedding_count: u32 LE (number of f32 values)
//!   - section_embeddings: f32 LE values
//!
//! Compressed format (`.ed`):
//! - `SAED` magic (4 bytes)
//! - version: u32 LE (1)
//! - model_id_len: u32 LE
//! - payload_len: u32 LE
//! - model_id: UTF-8 bytes
//! - payload: Brotli-compressed `SAGI` bytes

use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail};
use brotli::{CompressorReader, Decompressor};

use crate::bm25::Bm25Index;
use crate::chunk::ChunkMeta;
use crate::claims::ClaimEntry;
use crate::qa::QaEntry;

const MAGIC: &[u8; 4] = b"SAGI";
const VERSION: u32 = 4;
const ED_MAGIC: &[u8; 4] = b"SAED";
const ED_VERSION: u32 = 1;

const SECTION_QA: &str = "qa";
const SECTION_CLAIMS: &str = "claims";

/// A search index containing chunk metadata, embedding vectors, BM25 index,
/// chunk texts, and optional knowledge sections (qa/claims).
#[derive(Debug)]
pub struct SearchIndex {
    pub model_id: String,
    pub dim: usize,
    pub metadata: Vec<ChunkMeta>,
    /// Flat matrix: `metadata.len() * dim` f32 values, row-major.
    pub embeddings: Vec<f32>,
    pub bm25: Bm25Index,
    pub texts: Vec<String>,

    /// Optional QA section.
    pub qa_entries: Vec<QaEntry>,
    pub qa_dim: usize,
    pub qa_embeddings: Vec<f32>,

    /// Optional claims section.
    pub claims: Vec<ClaimEntry>,
    pub claim_dim: usize,
    pub claim_embeddings: Vec<f32>,
}

impl SearchIndex {
    /// Create a new index from chunks, their embeddings, BM25 index, and chunk texts.
    pub fn new(
        model_id: String,
        dim: usize,
        metadata: Vec<ChunkMeta>,
        embeddings: Vec<f32>,
        bm25: Bm25Index,
        texts: Vec<String>,
    ) -> Self {
        debug_assert_eq!(embeddings.len(), metadata.len() * dim);
        debug_assert!(texts.is_empty() || texts.len() == metadata.len());
        Self {
            model_id,
            dim,
            metadata,
            embeddings,
            bm25,
            texts,
            qa_entries: Vec::new(),
            qa_dim: 0,
            qa_embeddings: Vec::new(),
            claims: Vec::new(),
            claim_dim: 0,
            claim_embeddings: Vec::new(),
        }
    }

    pub fn with_qa_section(
        mut self,
        entries: Vec<QaEntry>,
        dim: usize,
        embeddings: Vec<f32>,
    ) -> Self {
        debug_assert!(entries.is_empty() || embeddings.len() == entries.len() * dim);
        self.qa_entries = entries;
        self.qa_dim = dim;
        self.qa_embeddings = embeddings;
        self
    }

    pub fn with_claims_section(
        mut self,
        claims: Vec<ClaimEntry>,
        dim: usize,
        embeddings: Vec<f32>,
    ) -> Self {
        debug_assert!(claims.is_empty() || embeddings.len() == claims.len() * dim);
        self.claims = claims;
        self.claim_dim = dim;
        self.claim_embeddings = embeddings;
        self
    }

    /// Serialize the index to a writer.
    pub fn write_to<W: Write>(&self, mut w: W) -> Result<()> {
        let model_id_bytes = self.model_id.as_bytes();
        let metadata_json =
            serde_json::to_vec(&self.metadata).context("serializing chunk metadata")?;

        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&(self.dim as u32).to_le_bytes())?;
        w.write_all(&(self.metadata.len() as u32).to_le_bytes())?;
        w.write_all(&(model_id_bytes.len() as u32).to_le_bytes())?;
        w.write_all(&(metadata_json.len() as u32).to_le_bytes())?;
        w.write_all(model_id_bytes)?;
        w.write_all(&metadata_json)?;

        for &val in &self.embeddings {
            w.write_all(&val.to_le_bytes())?;
        }

        // BM25 index trailer
        self.bm25.write_to(&mut w)?;

        // Chunk texts trailer
        w.write_all(&(self.texts.len() as u32).to_le_bytes())?;
        for text in &self.texts {
            let bytes = text.as_bytes();
            w.write_all(&(bytes.len() as u32).to_le_bytes())?;
            w.write_all(bytes)?;
        }

        // Knowledge sections
        let mut section_count = 0u32;
        if !self.qa_entries.is_empty() {
            section_count += 1;
        }
        if !self.claims.is_empty() {
            section_count += 1;
        }
        w.write_all(&section_count.to_le_bytes())?;

        if !self.qa_entries.is_empty() {
            write_section(
                &mut w,
                SECTION_QA,
                self.qa_entries.len(),
                &serde_json::to_vec(&self.qa_entries).context("serializing qa entries")?,
                self.qa_dim,
                &self.qa_embeddings,
            )?;
        }
        if !self.claims.is_empty() {
            write_section(
                &mut w,
                SECTION_CLAIMS,
                self.claims.len(),
                &serde_json::to_vec(&self.claims).context("serializing claims entries")?,
                self.claim_dim,
                &self.claim_embeddings,
            )?;
        }

        Ok(())
    }

    /// Serialize the index as a Brotli-compressed `.ed` payload.
    pub fn write_ed_to<W: Write>(&self, mut w: W) -> Result<()> {
        let mut raw = Vec::new();
        self.write_to(&mut raw)?;

        let compressed = brotli_compress(&raw).context("compressing SAGI payload with Brotli")?;
        let model_id_bytes = self.model_id.as_bytes();

        w.write_all(ED_MAGIC)?;
        w.write_all(&ED_VERSION.to_le_bytes())?;
        w.write_all(&(model_id_bytes.len() as u32).to_le_bytes())?;
        w.write_all(&(compressed.len() as u32).to_le_bytes())?;
        w.write_all(model_id_bytes)?;
        w.write_all(&compressed)?;
        Ok(())
    }

    /// Deserialize an index from a reader.
    pub fn read_from<R: Read>(mut r: R) -> Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic).context("reading magic bytes")?;
        if &magic != MAGIC {
            bail!(
                "invalid magic: expected SAGI, got {:?}",
                std::str::from_utf8(&magic).unwrap_or("<invalid>")
            );
        }

        let version = read_u32(&mut r).context("reading version")?;
        if version != VERSION {
            bail!(
                "unsupported index version: {} (expected {})",
                version,
                VERSION
            );
        }

        let dim = read_u32(&mut r).context("reading dim")? as usize;
        let num_chunks = read_u32(&mut r).context("reading num_chunks")? as usize;
        let model_id_len = read_u32(&mut r).context("reading model_id_len")? as usize;
        let metadata_len = read_u32(&mut r).context("reading metadata_len")? as usize;

        let mut model_id_bytes = vec![0u8; model_id_len];
        r.read_exact(&mut model_id_bytes)
            .context("reading model_id")?;
        let model_id = String::from_utf8(model_id_bytes).context("model_id is not valid UTF-8")?;

        let mut metadata_bytes = vec![0u8; metadata_len];
        r.read_exact(&mut metadata_bytes)
            .context("reading metadata")?;
        let metadata: Vec<ChunkMeta> =
            serde_json::from_slice(&metadata_bytes).context("parsing chunk metadata JSON")?;

        if metadata.len() != num_chunks {
            bail!(
                "metadata count mismatch: header says {} but JSON has {}",
                num_chunks,
                metadata.len()
            );
        }

        let total_floats = num_chunks * dim;
        let mut raw_bytes = vec![0u8; total_floats * 4];
        r.read_exact(&mut raw_bytes).context("reading embeddings")?;

        let embeddings: Vec<f32> = raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        // BM25 index trailer
        let bm25 = Bm25Index::read_from(&mut r).context("reading BM25 index")?;

        let text_count = read_u32(&mut r).context("reading text count")? as usize;
        let mut texts = Vec::with_capacity(text_count);
        for i in 0..text_count {
            let len = read_u32(&mut r)
                .with_context(|| format!("reading text length for chunk {}", i))?
                as usize;
            let mut text_bytes = vec![0u8; len];
            r.read_exact(&mut text_bytes)
                .with_context(|| format!("reading text for chunk {}", i))?;
            texts.push(
                String::from_utf8(text_bytes)
                    .with_context(|| format!("chunk {} text is not valid UTF-8", i))?,
            );
        }

        let section_count = read_u32(&mut r).context("reading section count")? as usize;
        let mut qa_entries: Vec<QaEntry> = Vec::new();
        let mut qa_dim = 0usize;
        let mut qa_embeddings: Vec<f32> = Vec::new();
        let mut claims: Vec<ClaimEntry> = Vec::new();
        let mut claim_dim = 0usize;
        let mut claim_embeddings: Vec<f32> = Vec::new();

        for _ in 0..section_count {
            let section = read_section(&mut r)?;
            match section.name.as_str() {
                SECTION_QA => {
                    qa_entries = serde_json::from_slice(&section.entries_json)
                        .context("parsing qa section json")?;
                    qa_dim = section.dim;
                    qa_embeddings = section.embeddings;
                    validate_section_embeddings("qa", qa_entries.len(), qa_dim, &qa_embeddings)?;
                }
                SECTION_CLAIMS => {
                    claims = serde_json::from_slice(&section.entries_json)
                        .context("parsing claims section json")?;
                    claim_dim = section.dim;
                    claim_embeddings = section.embeddings;
                    validate_section_embeddings(
                        "claims",
                        claims.len(),
                        claim_dim,
                        &claim_embeddings,
                    )?;
                }
                _ => {
                    // Unknown section: intentionally ignored.
                }
            }
        }

        Ok(Self {
            model_id,
            dim,
            metadata,
            embeddings,
            bm25,
            texts,
            qa_entries,
            qa_dim,
            qa_embeddings,
            claims,
            claim_dim,
            claim_embeddings,
        })
    }

    /// Extract model id from either a raw `SAGI` index or a compressed `.ed` payload.
    pub fn model_id_from_bytes(data: &[u8]) -> Result<String> {
        if data.len() < 4 {
            bail!("index bytes are too short");
        }

        if &data[..4] == ED_MAGIC {
            let (model_id, _) = parse_ed_container(data)?;
            return Ok(model_id);
        }
        if &data[..4] != MAGIC {
            bail!(
                "invalid index magic: expected SAGI or SAED, got {:?}",
                std::str::from_utf8(&data[..4]).unwrap_or("<invalid>")
            );
        }
        parse_sagi_model_id(data)
    }

    /// Deserialize an index from a byte slice.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() >= 4 && &data[..4] == ED_MAGIC {
            let (_, payload) = parse_ed_container(data)?;
            let raw = brotli_decompress(payload).context("decompressing .ed payload")?;
            return Self::read_from(Cursor::new(raw));
        }
        Self::read_from(Cursor::new(data))
    }

    /// Get the embedding vector for chunk at the given index.
    pub fn embedding(&self, index: usize) -> &[f32] {
        let start = index * self.dim;
        &self.embeddings[start..start + self.dim]
    }

    pub fn qa_embedding(&self, index: usize) -> Option<&[f32]> {
        if self.qa_dim == 0 || self.qa_embeddings.is_empty() {
            return None;
        }
        let start = index.checked_mul(self.qa_dim)?;
        let end = start.checked_add(self.qa_dim)?;
        self.qa_embeddings.get(start..end)
    }

    pub fn claim_embedding(&self, index: usize) -> Option<&[f32]> {
        if self.claim_dim == 0 || self.claim_embeddings.is_empty() {
            return None;
        }
        let start = index.checked_mul(self.claim_dim)?;
        let end = start.checked_add(self.claim_dim)?;
        self.claim_embeddings.get(start..end)
    }
}

struct EncodedSection {
    name: String,
    entries_json: Vec<u8>,
    dim: usize,
    embeddings: Vec<f32>,
}

fn validate_section_embeddings(
    section_name: &str,
    entry_count: usize,
    dim: usize,
    embeddings: &[f32],
) -> Result<()> {
    if entry_count == 0 {
        return Ok(());
    }
    if dim == 0 {
        bail!(
            "{} section has entries but zero embedding dim",
            section_name
        );
    }
    if embeddings.len() != entry_count * dim {
        bail!(
            "{} section embedding mismatch: {} entries * dim {} != {} floats",
            section_name,
            entry_count,
            dim,
            embeddings.len()
        );
    }
    Ok(())
}

fn write_section<W: Write>(
    w: &mut W,
    name: &str,
    item_count: usize,
    entries_json: &[u8],
    dim: usize,
    embeddings: &[f32],
) -> Result<()> {
    validate_section_embeddings(name, item_count, dim, embeddings)?;

    let name_bytes = name.as_bytes();
    w.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
    w.write_all(name_bytes)?;
    w.write_all(&(item_count as u32).to_le_bytes())?;
    w.write_all(&(entries_json.len() as u32).to_le_bytes())?;
    w.write_all(entries_json)?;
    w.write_all(&(dim as u32).to_le_bytes())?;
    w.write_all(&(embeddings.len() as u32).to_le_bytes())?;
    for &val in embeddings {
        w.write_all(&val.to_le_bytes())?;
    }
    Ok(())
}

fn read_section<R: Read>(r: &mut R) -> Result<EncodedSection> {
    let name_len = read_u32(r).context("reading section name length")? as usize;
    let mut name_bytes = vec![0u8; name_len];
    r.read_exact(&mut name_bytes)
        .context("reading section name")?;
    let name = String::from_utf8(name_bytes).context("section name is not UTF-8")?;

    let _item_count = read_u32(r).context("reading section item count")? as usize;
    let json_len = read_u32(r).context("reading section json length")? as usize;
    let mut entries_json = vec![0u8; json_len];
    r.read_exact(&mut entries_json)
        .context("reading section json")?;

    let dim = read_u32(r).context("reading section embedding dim")? as usize;
    let emb_count = read_u32(r).context("reading section embedding count")? as usize;
    let mut emb_raw = vec![0u8; emb_count * 4];
    r.read_exact(&mut emb_raw)
        .context("reading section embeddings")?;
    let embeddings = emb_raw
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();

    Ok(EncodedSection {
        name,
        entries_json,
        dim,
        embeddings,
    })
}

fn parse_sagi_model_id(data: &[u8]) -> Result<String> {
    if data.len() < 24 {
        bail!("SAGI payload is too short");
    }
    if &data[..4] != MAGIC {
        bail!("SAGI payload has invalid magic");
    }

    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if version != VERSION {
        bail!("unsupported SAGI version: {}", version);
    }

    let model_id_len = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
    let model_start = 24usize;
    let model_end = model_start
        .checked_add(model_id_len)
        .context("model id length overflow")?;

    if model_end > data.len() {
        bail!("SAGI payload truncated before model id");
    }

    String::from_utf8(data[model_start..model_end].to_vec()).context("model_id is not valid UTF-8")
}

fn parse_ed_container(data: &[u8]) -> Result<(String, &[u8])> {
    if data.len() < 16 {
        bail!("SAED payload is too short");
    }
    if &data[..4] != ED_MAGIC {
        bail!("SAED payload has invalid magic");
    }

    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if version != ED_VERSION {
        bail!(
            "unsupported SAED version: {} (expected {})",
            version,
            ED_VERSION
        );
    }

    let model_id_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let payload_len = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;

    let model_start = 16usize;
    let model_end = model_start
        .checked_add(model_id_len)
        .context("model id length overflow")?;
    if model_end > data.len() {
        bail!("SAED payload truncated before model id");
    }
    let payload_end = model_end
        .checked_add(payload_len)
        .context("payload length overflow")?;
    if payload_end > data.len() {
        bail!("SAED payload truncated before compressed index bytes");
    }

    let model_id = String::from_utf8(data[model_start..model_end].to_vec())
        .context("invalid SAED model id")?;
    Ok((model_id, &data[model_end..payload_end]))
}

fn brotli_compress(input: &[u8]) -> Result<Vec<u8>> {
    let mut reader = CompressorReader::new(Cursor::new(input), 16 * 1024, 11, 22);
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

fn brotli_decompress(input: &[u8]) -> Result<Vec<u8>> {
    let mut reader = Decompressor::new(Cursor::new(input), 16 * 1024);
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

fn read_u32<R: Read>(r: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_index() -> SearchIndex {
        let metadata = vec![
            ChunkMeta {
                title: "Test".to_string(),
                url: "/test/".to_string(),
                section: Some("Intro".to_string()),
                date: Some("2024-01-01".to_string()),
                granularity: Some("fine".to_string()),
                chunk_index: 0,
            },
            ChunkMeta {
                title: "Test".to_string(),
                url: "/test/".to_string(),
                section: Some("Body".to_string()),
                date: Some("2024-01-01".to_string()),
                granularity: Some("fine".to_string()),
                chunk_index: 1,
            },
        ];
        let embeddings = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2 chunks * 3 dims
        let texts = vec![
            "intro text here".to_string(),
            "body content here".to_string(),
        ];
        let bm25 = Bm25Index::build(&["intro text here", "body content here"]);
        let qa_entries = vec![QaEntry {
            question: "Who has the subject worked for?".to_string(),
            answer: "The subject has worked for Common Crawl.".to_string(),
            source_title: "About".to_string(),
            source_url: "/about/".to_string(),
            source_section: Some("Bio".to_string()),
            tags: vec!["work-history".to_string()],
            confidence: 0.9,
        }];
        let claims = vec![ClaimEntry {
            subject: "Subject".to_string(),
            predicate: "worked_for".to_string(),
            object: "Common Crawl".to_string(),
            evidence: "The subject worked for Common Crawl.".to_string(),
            source_title: "About".to_string(),
            source_url: "/about/".to_string(),
            source_section: Some("Bio".to_string()),
            tags: vec!["work-history".to_string()],
            confidence: 0.9,
        }];

        SearchIndex::new(
            "test-model".to_string(),
            3,
            metadata,
            embeddings,
            bm25,
            texts,
        )
        .with_qa_section(qa_entries, 3, vec![0.5, 0.4, 0.3])
        .with_claims_section(claims, 3, vec![0.2, 0.3, 0.4])
    }

    #[test]
    fn test_round_trip_with_sections() {
        let index = sample_index();
        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = SearchIndex::read_from(Cursor::new(&buf)).unwrap();
        assert_eq!(restored.model_id, "test-model");
        assert_eq!(restored.dim, 3);
        assert_eq!(restored.metadata.len(), 2);
        assert_eq!(restored.texts.len(), 2);
        assert_eq!(restored.qa_entries.len(), 1);
        assert_eq!(restored.qa_embeddings.len(), 3);
        assert_eq!(restored.claims.len(), 1);
        assert_eq!(restored.claim_embeddings.len(), 3);
    }

    #[test]
    fn test_ed_round_trip_from_bytes() {
        let index = sample_index();
        let mut buf = Vec::new();
        index.write_ed_to(&mut buf).unwrap();

        let restored = SearchIndex::from_bytes(&buf).unwrap();
        assert_eq!(restored.model_id, "test-model");
        assert_eq!(restored.qa_entries.len(), 1);
        assert_eq!(restored.claims.len(), 1);
        assert_eq!(restored.embedding(0), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_model_id_extraction_for_sagi_and_ed() {
        let index = sample_index();

        let mut raw = Vec::new();
        index.write_to(&mut raw).unwrap();
        assert_eq!(
            SearchIndex::model_id_from_bytes(&raw).unwrap(),
            "test-model"
        );

        let mut ed = Vec::new();
        index.write_ed_to(&mut ed).unwrap();
        assert_eq!(SearchIndex::model_id_from_bytes(&ed).unwrap(), "test-model");
    }

    #[test]
    fn test_empty_index() {
        let bm25 = Bm25Index::build(&[]);
        let index = SearchIndex::new(
            "model".to_string(),
            384,
            Vec::new(),
            Vec::new(),
            bm25,
            Vec::new(),
        );
        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = SearchIndex::read_from(Cursor::new(&buf)).unwrap();
        assert_eq!(restored.dim, 384);
        assert_eq!(restored.metadata.len(), 0);
        assert_eq!(restored.embeddings.len(), 0);
        assert_eq!(restored.bm25.num_docs, 0);
        assert_eq!(restored.texts.len(), 0);
        assert_eq!(restored.qa_entries.len(), 0);
        assert_eq!(restored.claims.len(), 0);
    }

    #[test]
    fn test_embedding_accessor() {
        let index = sample_index();
        assert_eq!(index.embedding(0), &[1.0, 2.0, 3.0]);
        assert_eq!(index.embedding(1), &[4.0, 5.0, 6.0]);
        assert_eq!(index.qa_embedding(0).unwrap(), &[0.5, 0.4, 0.3]);
        assert_eq!(index.claim_embedding(0).unwrap(), &[0.2, 0.3, 0.4]);
    }
}
