// SPDX-License-Identifier: GPL-3.0-only

//! Index format: serialize and deserialize the vector index + metadata.
//!
//! Binary format (v3):
//! - `SAGI` magic (4 bytes)
//! - version: u32 LE (3)
//! - dim: u32 LE (embedding dimensionality)
//! - num_chunks: u32 LE
//! - model_id_len: u32 LE
//! - metadata_len: u32 LE
//! - model_id: UTF-8 bytes
//! - metadata: JSON-encoded `Vec<ChunkMeta>` bytes
//! - embeddings: `num_chunks * dim` f32 values (LE)
//! - bm25_index: length-prefixed JSON (u32 LE length + JSON bytes)
//! - text_count: u32 LE (v3 only)
//! - texts: for each text, u32 LE length + UTF-8 bytes (v3 only)
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

const MAGIC: &[u8; 4] = b"SAGI";
const VERSION: u32 = 3;
const ED_MAGIC: &[u8; 4] = b"SAED";
const ED_VERSION: u32 = 1;

/// A search index containing chunk metadata, embedding vectors, BM25 index, and chunk texts.
#[derive(Debug)]
pub struct SearchIndex {
    pub model_id: String,
    pub dim: usize,
    pub metadata: Vec<ChunkMeta>,
    /// Flat matrix: `metadata.len() * dim` f32 values, row-major.
    pub embeddings: Vec<f32>,
    pub bm25: Bm25Index,
    /// Chunk texts for result snippets. Empty if loaded from a v2 index.
    pub texts: Vec<String>,
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
        }
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

        // Chunk texts trailer (v3)
        w.write_all(&(self.texts.len() as u32).to_le_bytes())?;
        for text in &self.texts {
            let bytes = text.as_bytes();
            w.write_all(&(bytes.len() as u32).to_le_bytes())?;
            w.write_all(bytes)?;
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

    /// Deserialize an index from a reader. Accepts v2 (texts will be empty) and v3.
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
        if version != 2 && version != 3 {
            bail!("unsupported index version: {} (expected 2 or 3)", version);
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

        // Chunk texts trailer (v3 only)
        let texts = if version >= 3 {
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
            texts
        } else {
            Vec::new()
        };

        Ok(Self {
            model_id,
            dim,
            metadata,
            embeddings,
            bm25,
            texts,
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
}

fn parse_sagi_model_id(data: &[u8]) -> Result<String> {
    if data.len() < 24 {
        bail!("SAGI payload is too short");
    }
    if &data[..4] != MAGIC {
        bail!("SAGI payload has invalid magic");
    }

    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if version != 2 && version != 3 {
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
                chunk_index: 0,
            },
            ChunkMeta {
                title: "Test".to_string(),
                url: "/test/".to_string(),
                section: Some("Body".to_string()),
                chunk_index: 1,
            },
        ];
        let embeddings = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 2 chunks * 3 dims
        let texts = vec![
            "intro text here".to_string(),
            "body content here".to_string(),
        ];
        let bm25 = Bm25Index::build(&["intro text here", "body content here"]);
        SearchIndex::new(
            "test-model".to_string(),
            3,
            metadata,
            embeddings,
            bm25,
            texts,
        )
    }

    #[test]
    fn test_round_trip() {
        let index = sample_index();
        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = SearchIndex::read_from(Cursor::new(&buf)).unwrap();
        assert_eq!(restored.model_id, "test-model");
        assert_eq!(restored.dim, 3);
        assert_eq!(restored.metadata.len(), 2);
        assert_eq!(restored.metadata[0].title, "Test");
        assert_eq!(restored.metadata[0].section.as_deref(), Some("Intro"));
        assert_eq!(restored.metadata[1].chunk_index, 1);
        assert_eq!(restored.embeddings, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(restored.bm25.num_docs, 2);
        assert_eq!(restored.texts.len(), 2);
        assert_eq!(restored.texts[0], "intro text here");
        assert_eq!(restored.texts[1], "body content here");
    }

    #[test]
    fn test_round_trip_from_bytes() {
        let index = sample_index();
        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = SearchIndex::from_bytes(&buf).unwrap();
        assert_eq!(restored.model_id, "test-model");
        assert_eq!(restored.texts.len(), 2);
    }

    #[test]
    fn test_ed_round_trip_from_bytes() {
        let index = sample_index();
        let mut buf = Vec::new();
        index.write_ed_to(&mut buf).unwrap();

        let restored = SearchIndex::from_bytes(&buf).unwrap();
        assert_eq!(restored.model_id, "test-model");
        assert_eq!(restored.dim, 3);
        assert_eq!(restored.texts.len(), 2);
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
    fn test_magic_validation() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"NOPE");
        buf.extend_from_slice(&[0u8; 100]);

        let err = SearchIndex::read_from(Cursor::new(&buf)).unwrap_err();
        assert!(
            format!("{}", err).contains("invalid magic"),
            "expected magic error, got: {}",
            err
        );
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
    }

    #[test]
    fn test_embedding_accessor() {
        let index = sample_index();
        assert_eq!(index.embedding(0), &[1.0, 2.0, 3.0]);
        assert_eq!(index.embedding(1), &[4.0, 5.0, 6.0]);
    }
}
