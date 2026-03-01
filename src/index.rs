// SPDX-License-Identifier: GPL-3.0-only

//! Index format: serialize and deserialize the vector index + metadata.
//!
//! Binary format (v2):
//! - `SAGI` magic (4 bytes)
//! - version: u32 LE (2)
//! - dim: u32 LE (embedding dimensionality)
//! - num_chunks: u32 LE
//! - model_id_len: u32 LE
//! - metadata_len: u32 LE
//! - model_id: UTF-8 bytes
//! - metadata: JSON-encoded `Vec<ChunkMeta>` bytes
//! - embeddings: `num_chunks * dim` f32 values (LE)
//! - bm25_index: length-prefixed JSON (u32 LE length + JSON bytes)

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};

use crate::bm25::Bm25Index;
use crate::chunk::ChunkMeta;

const MAGIC: &[u8; 4] = b"SAGI";
const VERSION: u32 = 2;

/// A search index containing chunk metadata, embedding vectors, and BM25 index.
#[derive(Debug)]
pub struct SearchIndex {
    pub model_id: String,
    pub dim: usize,
    pub metadata: Vec<ChunkMeta>,
    /// Flat matrix: `metadata.len() * dim` f32 values, row-major.
    pub embeddings: Vec<f32>,
    pub bm25: Bm25Index,
}

impl SearchIndex {
    /// Create a new index from chunks, their embeddings, and a BM25 index.
    pub fn new(
        model_id: String,
        dim: usize,
        metadata: Vec<ChunkMeta>,
        embeddings: Vec<f32>,
        bm25: Bm25Index,
    ) -> Self {
        debug_assert_eq!(embeddings.len(), metadata.len() * dim);
        Self {
            model_id,
            dim,
            metadata,
            embeddings,
            bm25,
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
            bail!("unsupported index version: {} (expected {})", version, VERSION);
        }

        let dim = read_u32(&mut r).context("reading dim")? as usize;
        let num_chunks = read_u32(&mut r).context("reading num_chunks")? as usize;
        let model_id_len = read_u32(&mut r).context("reading model_id_len")? as usize;
        let metadata_len = read_u32(&mut r).context("reading metadata_len")? as usize;

        let mut model_id_bytes = vec![0u8; model_id_len];
        r.read_exact(&mut model_id_bytes)
            .context("reading model_id")?;
        let model_id =
            String::from_utf8(model_id_bytes).context("model_id is not valid UTF-8")?;

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
        r.read_exact(&mut raw_bytes)
            .context("reading embeddings")?;

        let embeddings: Vec<f32> = raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        // BM25 index trailer
        let bm25 = Bm25Index::read_from(&mut r).context("reading BM25 index")?;

        Ok(Self {
            model_id,
            dim,
            metadata,
            embeddings,
            bm25,
        })
    }

    /// Get the embedding vector for chunk at the given index.
    pub fn embedding(&self, index: usize) -> &[f32] {
        let start = index * self.dim;
        &self.embeddings[start..start + self.dim]
    }
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
        let bm25 = Bm25Index::build(&["intro text here", "body content here"]);
        SearchIndex::new("test-model".to_string(), 3, metadata, embeddings, bm25)
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
        let index = SearchIndex::new("model".to_string(), 384, Vec::new(), Vec::new(), bm25);
        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = SearchIndex::read_from(Cursor::new(&buf)).unwrap();
        assert_eq!(restored.dim, 384);
        assert_eq!(restored.metadata.len(), 0);
        assert_eq!(restored.embeddings.len(), 0);
        assert_eq!(restored.bm25.num_docs, 0);
    }

    #[test]
    fn test_embedding_accessor() {
        let index = sample_index();
        assert_eq!(index.embedding(0), &[1.0, 2.0, 3.0]);
        assert_eq!(index.embedding(1), &[4.0, 5.0, 6.0]);
    }
}
