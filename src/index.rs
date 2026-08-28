// SPDX-License-Identifier: GPL-3.0-only

//! Index format v5: the `.ed` container and its sections.
//!
//! Container (`SAED` v2):
//!
//! ```text
//! "SAED" | u32 version=2 | u32 manifest_len | manifest JSON (uncompressed)
//!        | u32 payload_len | u32 payload_crc32 | u32 decompressed_len | brotli(payload)
//! ```
//!
//! The manifest is readable without decompressing anything ([`SearchIndex::manifest_from_bytes`]).
//! `decompressed_len` bounds the brotli output and `payload_crc32` is the
//! CRC-32 (IEEE) of the decompressed payload.
//!
//! Payload (`SAGI` v5): `"SAGI" | u32 version=5` followed by sections
//! `u32 name_len | name | u32 body_len | body`; unknown names are skipped.
//!
//! | section | body |
//! |---|---|
//! | `meta` | JSON `Vec<ChunkMeta>` |
//! | `texts` | `u32 n`, then `n × (u32 len, UTF-8, u16 overlap_words)`; texts are stored **without** their overlap prefix |
//! | `bm25` | see [`crate::bm25`] |
//! | `sparse` | `u32 terms`, per term `u32 token_id, f32 idf, u32 postings, (varint doc_delta, u16 weight×1000)*` |
//! | `dense/<scope>/<lane_id>` | `u8 quant (0=f32, 1=int8), u32 dim, u32 rows`, rows×dim values, then for int8 `rows × f32 scale`; scope ∈ `chunks`, `qa`, `claims` |
//! | `qa` | JSON `Vec<QaEntry>` |
//! | `claims` | JSON `Vec<ClaimEntry>` |
//!
//! Every length is checked against the remaining bytes before any
//! allocation, dims × rows use checked arithmetic (wasm32 is 32-bit), and
//! bm25/sparse document ids are validated against the chunk count. Output is
//! deterministic: sections are written in a fixed order and every dictionary
//! is sorted.

use std::collections::{BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail};
use brotli::{CompressorReader, Decompressor};

use crate::bm25::{Bm25Index, ByteCursor, write_varint};
use crate::chunk::ChunkMeta;
use crate::claims::ClaimEntry;
use crate::manifest::{
    Bm25Params, DenseSpec, FORMAT_VERSION, Manifest, Quant, SparseSpec, SparseTerm,
};
use crate::qa::QaEntry;

const ED_MAGIC: &[u8; 4] = b"SAED";
const ED_VERSION: u32 = 2;
const PAYLOAD_MAGIC: &[u8; 4] = b"SAGI";
const PAYLOAD_VERSION: u32 = FORMAT_VERSION;

const SECTION_META: &str = "meta";
const SECTION_TEXTS: &str = "texts";
const SECTION_BM25: &str = "bm25";
const SECTION_SPARSE: &str = "sparse";
const SECTION_QA: &str = "qa";
const SECTION_CLAIMS: &str = "claims";

/// Dense lane scopes.
pub const SCOPE_CHUNKS: &str = "chunks";
pub const SCOPE_QA: &str = "qa";
pub const SCOPE_CLAIMS: &str = "claims";

const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW: u32 = 22;

const LEGACY_HINT: &str =
    "legacy index format; rebuild the index with eddie 0.4 (`eddie index ...`)";

// ---------------------------------------------------------------------------
// Dense lanes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum LaneData {
    F32(Vec<f32>),
    Int8 { q: Vec<i8>, scales: Vec<f32> },
}

/// One dense vector matrix (`rows × dim`) for a lane, stored as f32 or as
/// symmetric per-row int8 with one f32 scale per row.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseLane {
    pub spec: DenseSpec,
    pub quant: Quant,
    pub dim: usize,
    pub rows: usize,
    data: LaneData,
}

impl DenseLane {
    /// Build a lane from row-major f32 values (`rows × dim`), quantising when
    /// `quant` is `Int8`. Rejects non-finite values and size mismatches.
    pub fn from_f32(
        spec: DenseSpec,
        dim: usize,
        rows: usize,
        values: &[f32],
        quant: Quant,
    ) -> Result<Self> {
        let expected = dim
            .checked_mul(rows)
            .context("dense lane dim × rows overflows")?;
        if values.len() != expected {
            bail!(
                "dense lane {:?}: {} values but dim {} × rows {} = {}",
                spec.id,
                values.len(),
                dim,
                rows,
                expected
            );
        }
        if rows > 0 && dim == 0 {
            bail!("dense lane {:?}: dim must be > 0", spec.id);
        }
        if values.iter().any(|v| !v.is_finite()) {
            bail!("dense lane {:?}: non-finite value", spec.id);
        }
        let data = match quant {
            Quant::F32 => LaneData::F32(values.to_vec()),
            Quant::Int8 => {
                let mut q = Vec::with_capacity(values.len());
                let mut scales = Vec::with_capacity(rows);
                for row in values.chunks_exact(dim.max(1)) {
                    let max_abs = row.iter().fold(0f32, |m, v| m.max(v.abs()));
                    let scale = max_abs / 127.0;
                    scales.push(scale);
                    if scale == 0.0 {
                        q.extend(std::iter::repeat_n(0i8, row.len()));
                    } else {
                        q.extend(
                            row.iter()
                                .map(|v| (v / scale).round().clamp(-127.0, 127.0) as i8),
                        );
                    }
                }
                LaneData::Int8 { q, scales }
            }
        };
        Ok(Self {
            spec,
            quant,
            dim,
            rows,
            data,
        })
    }

    /// Dot product of `query` with every row (cosine when both sides are
    /// L2-normalised). Errors when the query dimension does not match.
    pub fn scores(&self, query: &[f32]) -> Result<Vec<f32>> {
        if query.len() != self.dim {
            bail!(
                "query has {} dims but lane {:?} has {}",
                query.len(),
                self.spec.id,
                self.dim
            );
        }
        let mut out = Vec::with_capacity(self.rows);
        match &self.data {
            LaneData::F32(values) => {
                for row in values.chunks_exact(self.dim.max(1)) {
                    out.push(dot_f32(row, query));
                }
            }
            LaneData::Int8 { q, scales } => {
                for (row, &scale) in q.chunks_exact(self.dim.max(1)).zip(scales) {
                    let acc: f32 = row.iter().zip(query).map(|(&a, &b)| a as f32 * b).sum();
                    out.push(acc * scale);
                }
            }
        }
        Ok(out)
    }

    /// The `k` best rows by score, ties broken by ascending row index.
    pub fn top_k(&self, query: &[f32], k: usize) -> Result<Vec<(usize, f32)>> {
        let scores = self.scores(query)?;
        Ok(select_top_k(scores, k))
    }

    /// Dequantised copy of one row.
    pub fn row_f32(&self, row: usize) -> Option<Vec<f32>> {
        if row >= self.rows {
            return None;
        }
        let start = row * self.dim;
        Some(match &self.data {
            LaneData::F32(values) => values[start..start + self.dim].to_vec(),
            LaneData::Int8 { q, scales } => q[start..start + self.dim]
                .iter()
                .map(|&v| v as f32 * scales[row])
                .collect(),
        })
    }

    /// Serialized body size in bytes.
    pub fn byte_len(&self) -> usize {
        let cells = self.rows * self.dim;
        9 + match self.quant {
            Quant::F32 => cells * 4,
            Quant::Int8 => cells + self.rows * 4,
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.push(match self.quant {
            Quant::F32 => 0u8,
            Quant::Int8 => 1u8,
        });
        out.extend_from_slice(&(self.dim as u32).to_le_bytes());
        out.extend_from_slice(&(self.rows as u32).to_le_bytes());
        match &self.data {
            LaneData::F32(values) => {
                for v in values {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            LaneData::Int8 { q, scales } => {
                out.extend(q.iter().map(|&v| v as u8));
                for s in scales {
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        out
    }

    fn from_bytes(spec: DenseSpec, body: &[u8]) -> Result<Self> {
        let mut c = ByteCursor::new(body);
        let quant = match c.u8().context("dense quant")? {
            0 => Quant::F32,
            1 => Quant::Int8,
            other => bail!("dense lane {:?}: unknown quant {}", spec.id, other),
        };
        let dim = c.u32().context("dense dim")? as usize;
        let rows = c.u32().context("dense rows")? as usize;
        if rows > 0 && dim == 0 {
            bail!("dense lane {:?}: dim is 0 with {} rows", spec.id, rows);
        }
        let cells = dim
            .checked_mul(rows)
            .with_context(|| format!("dense lane {:?}: dim × rows overflows", spec.id))?;
        let data = match quant {
            Quant::F32 => {
                let nbytes = cells
                    .checked_mul(4)
                    .with_context(|| format!("dense lane {:?}: byte size overflows", spec.id))?;
                let raw = c.bytes(nbytes).context("dense f32 values")?;
                let mut values = Vec::with_capacity(cells);
                for chunk in raw.chunks_exact(4) {
                    let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if !v.is_finite() {
                        bail!("dense lane {:?}: non-finite value", spec.id);
                    }
                    values.push(v);
                }
                LaneData::F32(values)
            }
            Quant::Int8 => {
                let raw = c.bytes(cells).context("dense int8 values")?;
                let q: Vec<i8> = raw.iter().map(|&b| b as i8).collect();
                let scale_bytes = rows
                    .checked_mul(4)
                    .with_context(|| format!("dense lane {:?}: scale size overflows", spec.id))?;
                let raw = c.bytes(scale_bytes).context("dense int8 scales")?;
                let mut scales = Vec::with_capacity(rows);
                for chunk in raw.chunks_exact(4) {
                    let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if !s.is_finite() || s < 0.0 {
                        bail!("dense lane {:?}: invalid int8 scale", spec.id);
                    }
                    scales.push(s);
                }
                LaneData::Int8 { q, scales }
            }
        };
        if c.remaining() != 0 {
            bail!("dense lane {:?}: {} trailing bytes", spec.id, c.remaining());
        }
        Ok(Self {
            spec,
            quant,
            dim,
            rows,
            data,
        })
    }
}

fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Select the `k` highest scores; order is score descending, then index
/// ascending. Uses a partial selection so only the winners are sorted.
pub fn select_top_k(scores: Vec<f32>, k: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = scores.into_iter().enumerate().collect();
    if k == 0 || scored.is_empty() {
        return Vec::new();
    }
    let cmp = |a: &(usize, f32), b: &(usize, f32)| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0));
    if k < scored.len() {
        scored.select_nth_unstable_by(k - 1, cmp);
        scored.truncate(k);
    }
    scored.sort_by(cmp);
    scored
}

// ---------------------------------------------------------------------------
// Sparse index
// ---------------------------------------------------------------------------

/// Learned-sparse postings: per token id, its IDF (query-side weight) and the
/// documents that carry it with fixed-point weights (`u16 / 1000`).
#[derive(Debug, Clone, PartialEq)]
pub struct SparseIndex {
    pub num_docs: usize,
    /// Sorted token ids; `idf[i]` and `postings[i]` belong to `terms[i]`.
    pub terms: Vec<u32>,
    pub idf: Vec<f32>,
    postings: Vec<Vec<(u32, u16)>>,
}

/// Fixed-point scale for stored sparse weights.
pub const SPARSE_WEIGHT_SCALE: f32 = 1000.0;

impl SparseIndex {
    /// Build from per-document term lists. Terms with a non-positive weight or
    /// one that rounds to zero at 1/1000 resolution are dropped. A term with no
    /// entry in `idf` gets IDF 1.0 (neutral) so it stays searchable.
    pub fn build(docs: &[Vec<SparseTerm>], idf: &HashMap<u32, f32>) -> Self {
        let mut map: HashMap<u32, Vec<(u32, u16)>> = HashMap::new();
        for (doc_id, terms) in docs.iter().enumerate() {
            let mut seen: HashMap<u32, u16> = HashMap::new();
            for t in terms {
                let w = quantize_weight(t.weight);
                if w == 0 {
                    continue;
                }
                let e = seen.entry(t.token_id).or_default();
                *e = (*e).max(w);
            }
            let mut per_doc: Vec<(u32, u16)> = seen.into_iter().collect();
            per_doc.sort_unstable();
            for (token, w) in per_doc {
                map.entry(token).or_default().push((doc_id as u32, w));
            }
        }
        let mut terms: Vec<u32> = map.keys().copied().collect();
        terms.sort_unstable();
        let postings: Vec<Vec<(u32, u16)>> = terms
            .iter()
            .map(|t| map.remove(t).unwrap_or_default())
            .collect();
        let idf: Vec<f32> = terms
            .iter()
            .map(|t| idf.get(t).copied().filter(|v| v.is_finite()).unwrap_or(1.0))
            .collect();
        Self {
            num_docs: docs.len(),
            terms,
            idf,
            postings,
        }
    }

    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    fn slot(&self, token_id: u32) -> Option<usize> {
        self.terms.binary_search(&token_id).ok()
    }

    /// IDF stored for a token id, if the token occurs in the postings.
    pub fn idf_of(&self, token_id: u32) -> Option<f32> {
        self.slot(token_id).map(|i| self.idf[i])
    }

    /// Posting list `(doc_id, weight)` for a token id.
    pub fn postings_for(&self, token_id: u32) -> Vec<(usize, f32)> {
        match self.slot(token_id) {
            Some(i) => self.postings[i]
                .iter()
                .map(|&(d, w)| (d as usize, w as f32 / SPARSE_WEIGHT_SCALE))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Score = Σ query_weight × doc_weight over shared terms; the `k` best
    /// documents, ties by ascending doc id. Duplicate query terms are collapsed
    /// to their maximum weight.
    pub fn top_k(&self, query: &[SparseTerm], k: usize) -> Vec<(usize, f32)> {
        if k == 0 || self.num_docs == 0 || query.is_empty() {
            return Vec::new();
        }
        let mut qmax: HashMap<u32, f32> = HashMap::new();
        for t in query {
            if t.weight > 0.0 && t.weight.is_finite() {
                let e = qmax.entry(t.token_id).or_insert(0.0);
                *e = e.max(t.weight);
            }
        }
        let mut scores: HashMap<u32, f32> = HashMap::new();
        for (token, qw) in qmax {
            let Some(i) = self.slot(token) else {
                continue;
            };
            for &(doc, w) in &self.postings[i] {
                *scores.entry(doc).or_default() += qw * (w as f32 / SPARSE_WEIGHT_SCALE);
            }
        }
        let mut results: Vec<(usize, f32)> = scores
            .into_iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(d, s)| (d as usize, s))
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        results.truncate(k);
        results
    }

    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(self.terms.len())?.to_le_bytes());
        for ((&token, &idf), postings) in self.terms.iter().zip(&self.idf).zip(&self.postings) {
            out.extend_from_slice(&token.to_le_bytes());
            out.extend_from_slice(&idf.to_le_bytes());
            out.extend_from_slice(&u32::try_from(postings.len())?.to_le_bytes());
            let mut prev = 0u32;
            for (i, &(doc, w)) in postings.iter().enumerate() {
                let delta = if i == 0 { doc } else { doc - prev };
                write_varint(&mut out, delta);
                out.extend_from_slice(&w.to_le_bytes());
                prev = doc;
            }
        }
        Ok(out)
    }

    fn from_bytes(body: &[u8], num_docs: usize) -> Result<Self> {
        let mut c = ByteCursor::new(body);
        let term_count = c.u32().context("sparse term count")? as usize;
        let mut terms = Vec::with_capacity(term_count.min(c.remaining() / 12));
        let mut idf = Vec::with_capacity(terms.capacity());
        let mut postings = Vec::with_capacity(terms.capacity());
        for i in 0..term_count {
            let token = c.u32().with_context(|| format!("sparse term {} id", i))?;
            if let Some(&prev) = terms.last()
                && prev >= token
            {
                bail!("sparse term ids are not strictly sorted at {}", token);
            }
            let term_idf = c
                .f32()
                .with_context(|| format!("sparse term {} idf", token))?;
            if !term_idf.is_finite() {
                bail!("sparse term {} has non-finite idf", token);
            }
            let count = c
                .u32()
                .with_context(|| format!("sparse postings count for {}", token))?
                as usize;
            if count == 0 {
                bail!("sparse term {} has no postings", token);
            }
            let mut list = Vec::with_capacity(count.min(c.remaining() / 3));
            let mut prev = 0u32;
            for j in 0..count {
                let delta = c
                    .varint()
                    .with_context(|| format!("sparse posting {} of term {}", j, token))?;
                let doc = if j == 0 {
                    delta
                } else {
                    if delta == 0 {
                        bail!("sparse postings for {} are not strictly increasing", token);
                    }
                    prev.checked_add(delta).context("sparse doc id overflow")?
                };
                if doc as usize >= num_docs {
                    bail!(
                        "sparse posting doc id {} out of range (chunks {})",
                        doc,
                        num_docs
                    );
                }
                let w = c.u16().context("sparse weight")?;
                list.push((doc, w));
                prev = doc;
            }
            terms.push(token);
            idf.push(term_idf);
            postings.push(list);
        }
        if c.remaining() != 0 {
            bail!("sparse section has {} trailing bytes", c.remaining());
        }
        Ok(Self {
            num_docs,
            terms,
            idf,
            postings,
        })
    }
}

fn quantize_weight(w: f32) -> u16 {
    if !w.is_finite() || w <= 0.0 {
        return 0;
    }
    (w * SPARSE_WEIGHT_SCALE).round().min(u16::MAX as f32) as u16
}

// ---------------------------------------------------------------------------
// SearchIndex
// ---------------------------------------------------------------------------

/// A loaded index: chunk metadata and clean texts, the three retrieval arms,
/// and the optional qa/claims sections with their own dense lanes.
#[derive(Debug, Clone)]
pub struct SearchIndex {
    pub manifest: Manifest,
    pub metadata: Vec<ChunkMeta>,
    /// Chunk texts without their overlap prefix.
    pub texts: Vec<String>,
    /// Number of words that were prepended from the previous chunk when the
    /// chunk was embedded (informational; `texts` never contain them).
    pub overlap_words: Vec<u16>,
    pub bm25: Bm25Index,
    pub sparse: Option<SparseIndex>,
    /// Chunk-scope dense lanes, in manifest order.
    pub dense: Vec<DenseLane>,
    pub qa: Vec<QaEntry>,
    pub qa_dense: Vec<DenseLane>,
    pub claims: Vec<ClaimEntry>,
    pub claims_dense: Vec<DenseLane>,
}

/// Byte accounting for one payload section (see [`SearchIndex::inspect`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SectionInfo {
    pub name: String,
    pub raw_bytes: usize,
    /// Brotli size of the section body on its own (estimate; `None` when
    /// compression was not requested).
    pub compressed_bytes: Option<usize>,
}

/// Header and section sizes of an `.ed` file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IndexInfo {
    pub manifest: Manifest,
    pub file_bytes: usize,
    pub manifest_bytes: usize,
    pub payload_compressed_bytes: usize,
    pub payload_bytes: usize,
    pub sections: Vec<SectionInfo>,
}

impl SearchIndex {
    /// Serialize the index as a `SAED` v2 container.
    pub fn write_ed_to<W: Write>(&self, mut w: W) -> Result<()> {
        let manifest_json = serde_json::to_vec(&self.manifest).context("serializing manifest")?;
        let payload = self.payload_bytes()?;
        let crc = crc32(&payload);
        let compressed =
            brotli_compress(&payload, BROTLI_QUALITY).context("compressing payload")?;

        w.write_all(ED_MAGIC)?;
        w.write_all(&ED_VERSION.to_le_bytes())?;
        w.write_all(&len_u32(manifest_json.len(), "manifest")?.to_le_bytes())?;
        w.write_all(&manifest_json)?;
        w.write_all(&len_u32(compressed.len(), "compressed payload")?.to_le_bytes())?;
        w.write_all(&crc.to_le_bytes())?;
        w.write_all(&len_u32(payload.len(), "payload")?.to_le_bytes())?;
        w.write_all(&compressed)?;
        Ok(())
    }

    /// The uncompressed `SAGI` v5 payload.
    pub fn payload_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(PAYLOAD_MAGIC);
        out.extend_from_slice(&PAYLOAD_VERSION.to_le_bytes());

        write_section(
            &mut out,
            SECTION_META,
            &serde_json::to_vec(&self.metadata).context("serializing chunk metadata")?,
        )?;

        let mut texts = Vec::new();
        texts.extend_from_slice(&len_u32(self.texts.len(), "texts")?.to_le_bytes());
        for (text, &overlap) in self.texts.iter().zip(&self.overlap_words) {
            let bytes = text.as_bytes();
            texts.extend_from_slice(&len_u32(bytes.len(), "chunk text")?.to_le_bytes());
            texts.extend_from_slice(bytes);
            texts.extend_from_slice(&overlap.to_le_bytes());
        }
        write_section(&mut out, SECTION_TEXTS, &texts)?;

        write_section(&mut out, SECTION_BM25, &self.bm25.to_bytes()?)?;
        if let Some(sparse) = &self.sparse {
            write_section(&mut out, SECTION_SPARSE, &sparse.to_bytes()?)?;
        }
        for lane in &self.dense {
            write_section(
                &mut out,
                &lane_section_name(SCOPE_CHUNKS, &lane.spec.id),
                &lane.to_bytes(),
            )?;
        }
        if !self.qa.is_empty() {
            write_section(
                &mut out,
                SECTION_QA,
                &serde_json::to_vec(&self.qa).context("serializing qa entries")?,
            )?;
            for lane in &self.qa_dense {
                write_section(
                    &mut out,
                    &lane_section_name(SCOPE_QA, &lane.spec.id),
                    &lane.to_bytes(),
                )?;
            }
        }
        if !self.claims.is_empty() {
            write_section(
                &mut out,
                SECTION_CLAIMS,
                &serde_json::to_vec(&self.claims).context("serializing claims")?,
            )?;
            for lane in &self.claims_dense {
                write_section(
                    &mut out,
                    &lane_section_name(SCOPE_CLAIMS, &lane.spec.id),
                    &lane.to_bytes(),
                )?;
            }
        }
        Ok(out)
    }

    /// Parse only the uncompressed header of an `.ed` file.
    pub fn manifest_from_bytes(bytes: &[u8]) -> Result<Manifest> {
        Ok(parse_container(bytes)?.manifest)
    }

    /// Parse and validate a whole `.ed` file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let container = parse_container(bytes)?;
        let payload = brotli_decompress_exact(container.compressed, container.decompressed_len)
            .context("decompressing index payload")?;
        let actual_crc = crc32(&payload);
        if actual_crc != container.crc32 {
            bail!(
                "index payload CRC mismatch (header {:08x}, actual {:08x}); the file is corrupt",
                container.crc32,
                actual_crc
            );
        }
        Self::from_payload(container.manifest, &payload)
    }

    /// Section-level byte accounting without building the index. When
    /// `brotli_quality` is set, each section body is compressed on its own to
    /// estimate its share of the shipped size.
    pub fn inspect(bytes: &[u8], brotli_quality: Option<u32>) -> Result<IndexInfo> {
        let container = parse_container(bytes)?;
        let payload = brotli_decompress_exact(container.compressed, container.decompressed_len)
            .context("decompressing index payload")?;
        if crc32(&payload) != container.crc32 {
            bail!("index payload CRC mismatch; the file is corrupt");
        }
        let mut sections = Vec::new();
        for (name, body) in iter_sections(&payload)? {
            let compressed_bytes = match brotli_quality {
                Some(q) => Some(brotli_compress(body, q)?.len()),
                None => None,
            };
            sections.push(SectionInfo {
                name: name.to_string(),
                raw_bytes: body.len(),
                compressed_bytes,
            });
        }
        Ok(IndexInfo {
            manifest: container.manifest,
            file_bytes: bytes.len(),
            manifest_bytes: container.manifest_len,
            payload_compressed_bytes: container.compressed.len(),
            payload_bytes: payload.len(),
            sections,
        })
    }

    fn from_payload(manifest: Manifest, payload: &[u8]) -> Result<Self> {
        if manifest.format != FORMAT_VERSION {
            bail!(
                "unsupported manifest format {} (expected {}); {}",
                manifest.format,
                FORMAT_VERSION,
                LEGACY_HINT
            );
        }
        let chunks = manifest.chunks;

        let mut metadata: Option<Vec<ChunkMeta>> = None;
        let mut texts: Option<(Vec<String>, Vec<u16>)> = None;
        let mut bm25: Option<Bm25Index> = None;
        let mut sparse: Option<SparseIndex> = None;
        let mut qa: Vec<QaEntry> = Vec::new();
        let mut claims: Vec<ClaimEntry> = Vec::new();
        let mut lanes: Vec<(String, DenseLane)> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();

        for (name, body) in iter_sections(payload)? {
            if !seen.insert(name.to_string()) {
                bail!("duplicate section {:?}", name);
            }
            match name {
                SECTION_META => {
                    let parsed: Vec<ChunkMeta> =
                        serde_json::from_slice(body).context("parsing chunk metadata JSON")?;
                    if parsed.len() != chunks {
                        bail!(
                            "manifest says {} chunks but meta has {}",
                            chunks,
                            parsed.len()
                        );
                    }
                    metadata = Some(parsed);
                }
                SECTION_TEXTS => {
                    let mut c = ByteCursor::new(body);
                    let n = c.u32().context("text count")? as usize;
                    if n != chunks {
                        bail!("manifest says {} chunks but texts has {}", chunks, n);
                    }
                    let mut list = Vec::with_capacity(n.min(c.remaining() / 6));
                    let mut overlaps = Vec::with_capacity(list.capacity());
                    for i in 0..n {
                        let len = c.u32().with_context(|| format!("text {} length", i))? as usize;
                        let raw = c.bytes(len).with_context(|| format!("text {}", i))?;
                        let text = std::str::from_utf8(raw)
                            .with_context(|| format!("chunk {} text is not valid UTF-8", i))?;
                        list.push(text.to_string());
                        overlaps.push(c.u16().with_context(|| format!("text {} overlap", i))?);
                    }
                    if c.remaining() != 0 {
                        bail!("texts section has {} trailing bytes", c.remaining());
                    }
                    texts = Some((list, overlaps));
                }
                SECTION_BM25 => {
                    bm25 = Some(Bm25Index::from_bytes(body, chunks, manifest.bm25)?);
                }
                SECTION_SPARSE => {
                    sparse = Some(SparseIndex::from_bytes(body, chunks)?);
                }
                SECTION_QA => {
                    qa = serde_json::from_slice(body).context("parsing qa section JSON")?;
                }
                SECTION_CLAIMS => {
                    claims = serde_json::from_slice(body).context("parsing claims section JSON")?;
                }
                other => {
                    if let Some((scope, lane_id)) =
                        parse_lane_section_name(other).filter(|(scope, _)| {
                            matches!(*scope, SCOPE_CHUNKS | SCOPE_QA | SCOPE_CLAIMS)
                        })
                    {
                        let spec = manifest.dense_lane(lane_id).with_context(|| {
                            format!(
                                "dense section {:?} names a lane missing from the manifest",
                                other
                            )
                        })?;
                        let lane = DenseLane::from_bytes(spec.clone(), body)
                            .with_context(|| format!("reading section {:?}", other))?;
                        if lane.dim != spec.dim {
                            bail!(
                                "section {:?} has dim {} but manifest lane {:?} has {}",
                                other,
                                lane.dim,
                                spec.id,
                                spec.dim
                            );
                        }
                        if lane.quant != spec.quant {
                            bail!(
                                "section {:?} quant {:?} differs from manifest {:?}",
                                other,
                                lane.quant,
                                spec.quant
                            );
                        }
                        lanes.push((scope.to_string(), lane));
                    }
                    // Any other name is an unknown section: skipped.
                }
            }
        }

        let metadata = metadata.context("index has no meta section")?;
        let (texts, overlap_words) = texts.context("index has no texts section")?;
        let bm25 = bm25.context("index has no bm25 section")?;

        match (&manifest.sparse, &sparse) {
            (Some(spec), Some(idx)) => {
                if spec.terms != idx.term_count() {
                    bail!(
                        "manifest says {} sparse terms but section has {}",
                        spec.terms,
                        idx.term_count()
                    );
                }
            }
            (Some(_), None) => {
                bail!("manifest declares a sparse arm but the payload has no sparse section")
            }
            (None, Some(_)) => bail!("payload has a sparse section the manifest does not declare"),
            (None, None) => {}
        }

        let mut dense = Vec::new();
        let mut qa_dense = Vec::new();
        let mut claims_dense = Vec::new();
        for (scope, lane) in lanes {
            let (expected_rows, what) = match scope.as_str() {
                SCOPE_CHUNKS => (chunks, "chunks"),
                SCOPE_QA => (qa.len(), "qa entries"),
                SCOPE_CLAIMS => (claims.len(), "claims"),
                _ => continue,
            };
            if lane.rows != expected_rows {
                bail!(
                    "dense/{}/{} has {} rows but there are {} {}",
                    scope,
                    lane.spec.id,
                    lane.rows,
                    expected_rows,
                    what
                );
            }
            match scope.as_str() {
                SCOPE_CHUNKS => dense.push(lane),
                SCOPE_QA => qa_dense.push(lane),
                _ => claims_dense.push(lane),
            }
        }
        // Keep chunk lanes in manifest order and require one section per lane.
        dense.sort_by_key(|l| manifest.dense.iter().position(|s| s.id == l.spec.id));
        for spec in &manifest.dense {
            if !dense.iter().any(|l| l.spec.id == spec.id) {
                bail!(
                    "manifest lane {:?} has no dense/chunks/{} section",
                    spec.id,
                    spec.id
                );
            }
        }

        Ok(Self {
            manifest,
            metadata,
            texts,
            overlap_words,
            bm25,
            sparse,
            dense,
            qa,
            qa_dense,
            claims,
            claims_dense,
        })
    }

    /// Chunk ids for a page URL, ordered by `chunk_index` (then id).
    pub fn page_chunks(&self, url: &str) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .metadata
            .iter()
            .enumerate()
            .filter(|(_, m)| m.url == url)
            .map(|(i, _)| i)
            .collect();
        ids.sort_by_key(|&i| (self.metadata[i].chunk_index, i));
        ids
    }

    /// Position of a chunk-scope lane by id.
    pub fn dense_lane(&self, id: &str) -> Option<usize> {
        self.dense.iter().position(|l| l.spec.id == id)
    }

    pub fn qa_lane(&self, id: &str) -> Option<&DenseLane> {
        self.qa_dense.iter().find(|l| l.spec.id == id)
    }

    pub fn claims_lane(&self, id: &str) -> Option<&DenseLane> {
        self.claims_dense.iter().find(|l| l.spec.id == id)
    }
}

// ---------------------------------------------------------------------------
// IndexBuilder
// ---------------------------------------------------------------------------

/// Assembles a [`SearchIndex`] from its parts and derives the manifest.
#[derive(Debug, Default)]
pub struct IndexBuilder {
    metadata: Vec<ChunkMeta>,
    texts: Vec<String>,
    overlap_words: Vec<u16>,
    bm25_params: Bm25Params,
    sparse: Option<(SparseIndex, SparseSpec)>,
    dense: Vec<DenseLane>,
    qa: Vec<QaEntry>,
    qa_dense: Vec<DenseLane>,
    claims: Vec<ClaimEntry>,
    claims_dense: Vec<DenseLane>,
    built_at: Option<String>,
}

impl IndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add the chunks. `texts[i]` is the text exactly as it was embedded; it
    /// may begin with `overlap_words[i]` whitespace-delimited words copied from
    /// the previous chunk, which the builder strips so the stored text, BM25
    /// and snippets only see the chunk's own content. Pass 0 for chunks that
    /// carry no overlap prefix (or whose text is already clean).
    pub fn add_chunks(
        &mut self,
        metadata: Vec<ChunkMeta>,
        texts: Vec<String>,
        overlap_words: Vec<u16>,
    ) -> Result<&mut Self> {
        if texts.len() != metadata.len() || overlap_words.len() != metadata.len() {
            bail!(
                "add_chunks: {} metadata, {} texts, {} overlaps",
                metadata.len(),
                texts.len(),
                overlap_words.len()
            );
        }
        let clean: Vec<String> = texts
            .iter()
            .zip(&overlap_words)
            .map(|(t, &o)| strip_leading_words(t, o as usize).to_string())
            .collect();
        self.metadata = metadata;
        self.texts = clean;
        self.overlap_words = overlap_words;
        Ok(self)
    }

    /// BM25 parameters (defaults: k1 1.2, b 0.75). The arm is always built.
    pub fn bm25_params(&mut self, params: Bm25Params) -> &mut Self {
        self.bm25_params = params;
        self
    }

    /// Add the learned-sparse arm from per-chunk term lists and the IDF table
    /// of the encoder's vocabulary. `spec.terms` is overwritten with the number
    /// of distinct terms actually stored.
    pub fn add_sparse(
        &mut self,
        docs: &[Vec<SparseTerm>],
        idf: &HashMap<u32, f32>,
        mut spec: SparseSpec,
    ) -> Result<&mut Self> {
        if docs.len() != self.metadata.len() {
            bail!(
                "add_sparse: {} documents but {} chunks (call add_chunks first)",
                docs.len(),
                self.metadata.len()
            );
        }
        let index = SparseIndex::build(docs, idf);
        spec.terms = index.term_count();
        self.sparse = Some((index, spec));
        Ok(self)
    }

    /// Add a dense lane for `scope` (`chunks`, `qa`, or `claims`). Row counts
    /// are checked against the scope's entries in [`IndexBuilder::finish`], so
    /// lanes and entries may be added in any order. Lanes for `qa`/`claims`
    /// must reuse the id of a `chunks` lane (same model).
    pub fn add_dense_lane(&mut self, scope: &str, lane: DenseLane) -> Result<&mut Self> {
        if lane.spec.id.is_empty() || lane.spec.id.contains('/') {
            bail!(
                "dense lane id {:?} must be non-empty and contain no '/'",
                lane.spec.id
            );
        }
        let target = match scope {
            SCOPE_CHUNKS => &mut self.dense,
            SCOPE_QA => &mut self.qa_dense,
            SCOPE_CLAIMS => &mut self.claims_dense,
            other => bail!("unknown dense scope {:?}", other),
        };
        if target.iter().any(|l| l.spec.id == lane.spec.id) {
            bail!("dense/{}/{} added twice", scope, lane.spec.id);
        }
        target.push(lane);
        Ok(self)
    }

    pub fn add_qa(&mut self, entries: Vec<QaEntry>) -> &mut Self {
        self.qa = entries;
        self
    }

    pub fn add_claims(&mut self, entries: Vec<ClaimEntry>) -> &mut Self {
        self.claims = entries;
        self
    }

    /// RFC 3339 build timestamp for the manifest. Leave unset for
    /// byte-reproducible builds.
    pub fn built_at(&mut self, ts: Option<String>) -> &mut Self {
        self.built_at = ts;
        self
    }

    pub fn finish(self) -> Result<SearchIndex> {
        let chunks = self.metadata.len();
        let refs: Vec<&str> = self.texts.iter().map(String::as_str).collect();
        let bm25 = Bm25Index::build_with_params(&refs, self.bm25_params);

        for lane in &self.dense {
            check_rows(SCOPE_CHUNKS, lane, chunks)?;
        }
        for lane in &self.qa_dense {
            check_rows(SCOPE_QA, lane, self.qa.len())?;
            if !self.dense.iter().any(|l| l.spec.id == lane.spec.id) {
                bail!("qa lane {:?} has no matching chunks lane", lane.spec.id);
            }
        }
        for lane in &self.claims_dense {
            check_rows(SCOPE_CLAIMS, lane, self.claims.len())?;
            if !self.dense.iter().any(|l| l.spec.id == lane.spec.id) {
                bail!("claims lane {:?} has no matching chunks lane", lane.spec.id);
            }
        }

        let pages = self
            .metadata
            .iter()
            .map(|m| m.url.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let mut manifest = Manifest::new(chunks, pages);
        manifest.dense = self
            .dense
            .iter()
            .map(|l| {
                let mut spec = l.spec.clone();
                spec.dim = l.dim;
                spec.quant = l.quant;
                spec
            })
            .collect();
        manifest.bm25 = self.bm25_params;
        manifest.built_at = self.built_at;
        let (sparse, sparse_spec) = match self.sparse {
            Some((idx, spec)) => (Some(idx), Some(spec)),
            None => (None, None),
        };
        manifest.sparse = sparse_spec;
        if !self.qa.is_empty() {
            manifest.sections.push(SECTION_QA.to_string());
        }
        if !self.claims.is_empty() {
            manifest.sections.push(SECTION_CLAIMS.to_string());
        }

        let mut index = SearchIndex {
            manifest,
            metadata: self.metadata,
            texts: self.texts,
            overlap_words: self.overlap_words,
            bm25,
            sparse,
            dense: self.dense,
            qa: self.qa,
            qa_dense: self.qa_dense,
            claims: self.claims,
            claims_dense: self.claims_dense,
        };
        // Lanes for empty sections are dropped (nothing to score).
        if index.qa.is_empty() {
            index.qa_dense.clear();
        }
        if index.claims.is_empty() {
            index.claims_dense.clear();
        }
        Ok(index)
    }
}

fn check_rows(scope: &str, lane: &DenseLane, expected: usize) -> Result<()> {
    if lane.rows != expected {
        bail!(
            "dense/{}/{} has {} rows but the scope has {} entries",
            scope,
            lane.spec.id,
            lane.rows,
            expected
        );
    }
    Ok(())
}

/// Skip the first `n` whitespace-delimited words (and the whitespace after
/// them). Returns the whole text when it has `n` words or fewer.
pub fn strip_leading_words(text: &str, n: usize) -> &str {
    if n == 0 {
        return text;
    }
    let mut words = 0usize;
    let mut in_word = false;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            in_word = false;
        } else {
            if !in_word {
                if words == n {
                    return &text[i..];
                }
                words += 1;
            }
            in_word = true;
        }
    }
    ""
}

// ---------------------------------------------------------------------------
// Container and sections
// ---------------------------------------------------------------------------

struct Container<'a> {
    manifest: Manifest,
    manifest_len: usize,
    compressed: &'a [u8],
    crc32: u32,
    decompressed_len: usize,
}

fn parse_container(bytes: &[u8]) -> Result<Container<'_>> {
    let mut c = ByteCursor::new(bytes);
    let magic = c.bytes(4).context("index is too short for a header")?;
    if magic != ED_MAGIC {
        if magic == PAYLOAD_MAGIC {
            bail!(
                "bare SAGI payload without an .ed container; {}",
                LEGACY_HINT
            );
        }
        bail!(
            "not an eddie index (magic {:?})",
            String::from_utf8_lossy(magic)
        );
    }
    let version = c.u32().context("container version")?;
    if version != ED_VERSION {
        bail!(
            "unsupported .ed container version {} (expected {}); {}",
            version,
            ED_VERSION,
            LEGACY_HINT
        );
    }
    let manifest_len = c.u32().context("manifest length")? as usize;
    let manifest_bytes = c.bytes(manifest_len).context("manifest bytes")?;
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).context("parsing manifest JSON")?;
    if manifest.format != FORMAT_VERSION {
        bail!(
            "unsupported index format {} (expected {}); {}",
            manifest.format,
            FORMAT_VERSION,
            LEGACY_HINT
        );
    }
    let payload_len = c.u32().context("payload length")? as usize;
    let crc32 = c.u32().context("payload crc32")?;
    let decompressed_len = c.u32().context("decompressed length")? as usize;
    let compressed = c.bytes(payload_len).context("compressed payload")?;
    if c.remaining() != 0 {
        bail!("{} trailing bytes after the payload", c.remaining());
    }
    Ok(Container {
        manifest,
        manifest_len,
        compressed,
        crc32,
        decompressed_len,
    })
}

fn iter_sections(payload: &[u8]) -> Result<Vec<(&str, &[u8])>> {
    let mut c = ByteCursor::new(payload);
    let magic = c.bytes(4).context("payload magic")?;
    if magic != PAYLOAD_MAGIC {
        bail!("payload magic is not SAGI");
    }
    let version = c.u32().context("payload version")?;
    if version != PAYLOAD_VERSION {
        bail!(
            "unsupported payload version {} (expected {}); {}",
            version,
            PAYLOAD_VERSION,
            LEGACY_HINT
        );
    }
    let mut out = Vec::new();
    while c.remaining() > 0 {
        let at = c.position();
        let name_len =
            c.u32()
                .with_context(|| format!("section name length at {}", at))? as usize;
        let name = std::str::from_utf8(c.bytes(name_len).context("section name")?)
            .context("section name is not UTF-8")?;
        let body_len =
            c.u32()
                .with_context(|| format!("section {:?} body length", name))? as usize;
        let body = c
            .bytes(body_len)
            .with_context(|| format!("section {:?} body", name))?;
        out.push((name, body));
    }
    Ok(out)
}

fn write_section(out: &mut Vec<u8>, name: &str, body: &[u8]) -> Result<()> {
    out.extend_from_slice(&len_u32(name.len(), "section name")?.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&len_u32(body.len(), name)?.to_le_bytes());
    out.extend_from_slice(body);
    Ok(())
}

fn lane_section_name(scope: &str, lane_id: &str) -> String {
    format!("dense/{}/{}", scope, lane_id)
}

fn parse_lane_section_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("dense/")?;
    let (scope, lane) = rest.split_once('/')?;
    if lane.is_empty() || lane.contains('/') {
        return None;
    }
    Some((scope, lane))
}

fn len_u32(len: usize, what: &str) -> Result<u32> {
    u32::try_from(len).with_context(|| format!("{} exceeds 4 GiB", what))
}

fn brotli_compress(input: &[u8], quality: u32) -> Result<Vec<u8>> {
    let mut reader = CompressorReader::new(Cursor::new(input), 16 * 1024, quality, BROTLI_WINDOW);
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

/// Decompress exactly `expected_len` bytes. The reader is capped one byte past
/// the declared length so a hostile stream cannot grow memory without bound,
/// and the allocation uses `try_reserve` so an oversized claim fails instead
/// of aborting the process.
fn brotli_decompress_exact(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(expected_len).with_context(|| {
        format!(
            "cannot allocate {} bytes for the index payload",
            expected_len
        )
    })?;
    let mut reader = Decompressor::new(Cursor::new(input), 16 * 1024).take(expected_len as u64 + 1);
    reader.read_to_end(&mut out)?;
    if out.len() != expected_len {
        bail!(
            "payload decompressed to {} bytes, header says {}",
            if out.len() > expected_len {
                "more than the declared".to_string()
            } else {
                out.len().to_string()
            },
            expected_len
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CRC-32 (IEEE 802.3, reflected)
// ---------------------------------------------------------------------------

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// CRC-32 as used by zlib/PNG.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// Test helpers shared with search.rs
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::manifest::{Family, Pooling, RuntimeSpec};

    /// xorshift64* PRNG so tests are reproducible without a dependency.
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        pub fn unit(&mut self) -> f32 {
            (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
        }
        /// Approximately normal via sum of uniforms.
        pub fn normal(&mut self) -> f32 {
            (0..6).map(|_| self.unit()).sum::<f32>() - 3.0
        }
        pub fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }
    }

    pub fn normalize(v: &mut [f32]) {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v.iter_mut().for_each(|x| *x /= n);
    }

    pub fn wasm_spec(id: &str, dim: usize, quant: Quant) -> DenseSpec {
        DenseSpec {
            id: id.to_string(),
            model: "sentence-transformers/multi-qa-MiniLM-L6-cos-v1".to_string(),
            family: Family::Bert,
            dim,
            pooling: Pooling::Mean,
            normalize: true,
            query_prefix: String::new(),
            doc_prefix: String::new(),
            max_seq_len: 512,
            revision: None,
            quant,
            runtime: RuntimeSpec::WasmCandle {
                files: vec![
                    "config.json".into(),
                    "tokenizer.json".into(),
                    "model.safetensors".into(),
                ],
            },
        }
    }

    pub struct Synthetic {
        pub metadata: Vec<ChunkMeta>,
        pub texts: Vec<String>,
        pub vectors: Vec<f32>,
        pub sparse_docs: Vec<Vec<SparseTerm>>,
        pub idf: HashMap<u32, f32>,
        pub dim: usize,
    }

    /// A clustered synthetic corpus: `n` chunks over `n / 8` pages, 40 topic
    /// clusters, ~120 words of text and ~120 sparse terms per chunk.
    pub fn synthetic_corpus(n: usize, dim: usize, seed: u64) -> Synthetic {
        let mut rng = Rng(seed | 1);
        let clusters = 40usize;
        let mut centers: Vec<Vec<f32>> = Vec::new();
        for _ in 0..clusters {
            let mut c: Vec<f32> = (0..dim).map(|_| rng.normal()).collect();
            normalize(&mut c);
            centers.push(c);
        }
        let mut metadata = Vec::with_capacity(n);
        let mut texts = Vec::with_capacity(n);
        let mut vectors = Vec::with_capacity(n * dim);
        let mut sparse_docs = Vec::with_capacity(n);
        let mut idf = HashMap::new();
        let chunks_per_page = 8usize;
        for i in 0..n {
            let cluster = i % clusters;
            let page = i / chunks_per_page;
            let mut v: Vec<f32> = centers[cluster]
                .iter()
                .map(|c| c + 0.6 * rng.normal())
                .collect();
            normalize(&mut v);
            vectors.extend_from_slice(&v);

            let mut words = Vec::with_capacity(120);
            for _ in 0..100 {
                words.push(format!("w{}", rng.below(3000)));
            }
            for _ in 0..20 {
                words.push(format!("topic{}x{}", cluster, rng.below(12)));
            }
            let text = format!(
                "Chunk {} of page {} covers topic{}. {}. It ends with a closing sentence about topic{} facts.",
                i,
                page,
                cluster,
                words.join(" "),
                cluster
            );
            texts.push(text);

            metadata.push(ChunkMeta {
                title: format!("Page {}", page),
                url: format!("/page-{}/", page),
                section: Some(format!("Section {}", i % chunks_per_page)),
                date: if page.is_multiple_of(3) {
                    None
                } else {
                    Some(format!("20{:02}-01-01", 10 + page % 15))
                },
                granularity: Some(
                    if i % chunks_per_page == 7 {
                        "coarse"
                    } else {
                        "fine"
                    }
                    .to_string(),
                ),
                chunk_index: i % chunks_per_page,
            });

            let mut terms = Vec::with_capacity(120);
            for _ in 0..100 {
                terms.push(SparseTerm {
                    token_id: rng.below(30_000) as u32,
                    weight: 0.1 + rng.unit() * 2.0,
                });
            }
            for k in 0..20 {
                terms.push(SparseTerm {
                    token_id: 30_000 + (cluster * 20 + k) as u32,
                    weight: 1.0 + rng.unit(),
                });
            }
            terms.sort_by_key(|t| t.token_id);
            for t in &terms {
                idf.entry(t.token_id)
                    .or_insert_with(|| 1.0 + (t.token_id % 7) as f32);
            }
            sparse_docs.push(terms);
        }
        Synthetic {
            metadata,
            texts,
            vectors,
            sparse_docs,
            idf,
            dim,
        }
    }

    pub fn build_synthetic_index(
        n: usize,
        dim: usize,
        quant: Quant,
        with_sparse: bool,
    ) -> (SearchIndex, Synthetic) {
        let corpus = synthetic_corpus(n, dim, 0x5EED);
        let mut builder = IndexBuilder::new();
        builder
            .add_chunks(corpus.metadata.clone(), corpus.texts.clone(), vec![0; n])
            .unwrap();
        builder
            .add_dense_lane(
                SCOPE_CHUNKS,
                DenseLane::from_f32(
                    wasm_spec("minilm", dim, quant),
                    dim,
                    n,
                    &corpus.vectors,
                    quant,
                )
                .unwrap(),
            )
            .unwrap();
        if with_sparse {
            builder
                .add_sparse(
                    &corpus.sparse_docs,
                    &corpus.idf,
                    SparseSpec {
                        model:
                            "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill"
                                .into(),
                        tokenizer:
                            "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill"
                                .into(),
                        revision: None,
                        vocab_hash: "00".into(),
                        terms: 0,
                    },
                )
                .unwrap();
        }
        (builder.finish().unwrap(), corpus)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn meta(url: &str, idx: usize, gran: &str) -> ChunkMeta {
        ChunkMeta {
            title: format!("Title {}", url),
            url: url.to_string(),
            section: Some(format!("S{}", idx)),
            date: Some("2024-01-01".to_string()),
            granularity: Some(gran.to_string()),
            chunk_index: idx,
        }
    }

    fn sample_index() -> SearchIndex {
        let metadata = vec![
            meta("/a/", 0, "fine"),
            meta("/a/", 1, "fine"),
            meta("/b/", 0, "coarse"),
        ];
        let texts = vec![
            "intro text here about rust".to_string(),
            "here about rust body content continues".to_string(),
            "python scripting page".to_string(),
        ];
        let vectors = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let mut b = IndexBuilder::new();
        b.add_chunks(metadata, texts, vec![0, 2, 0]).unwrap();
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(
                wasm_spec("minilm", 3, Quant::F32),
                3,
                3,
                &vectors,
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        let mut idf = HashMap::new();
        idf.insert(7u32, 2.5f32);
        b.add_sparse(
            &[
                vec![
                    SparseTerm {
                        token_id: 7,
                        weight: 1.2,
                    },
                    SparseTerm {
                        token_id: 9,
                        weight: 0.4,
                    },
                ],
                vec![SparseTerm {
                    token_id: 7,
                    weight: 0.5,
                }],
                vec![],
            ],
            &idf,
            SparseSpec {
                model: "m".into(),
                tokenizer: "t".into(),
                revision: Some("abc".into()),
                vocab_hash: "ff".into(),
                terms: 0,
            },
        )
        .unwrap();
        b.add_qa(vec![QaEntry {
            question: "Who?".into(),
            answer: "Them.".into(),
            source_title: "A".into(),
            source_url: "/a/".into(),
            source_section: None,
            tags: vec![],
            confidence: 0.9,
        }]);
        b.add_dense_lane(
            SCOPE_QA,
            DenseLane::from_f32(
                wasm_spec("minilm", 3, Quant::F32),
                3,
                1,
                &[0.0, 1.0, 0.0],
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        b.add_claims(vec![ClaimEntry {
            subject: "S".into(),
            predicate: "p".into(),
            object: "o".into(),
            evidence: "e".into(),
            source_title: "A".into(),
            source_url: "/a/".into(),
            source_section: None,
            tags: vec![],
            confidence: 0.5,
        }]);
        b.add_dense_lane(
            SCOPE_CLAIMS,
            DenseLane::from_f32(
                wasm_spec("minilm", 3, Quant::F32),
                3,
                1,
                &[0.0, 0.0, 1.0],
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        b.finish().unwrap()
    }

    fn ed_bytes(index: &SearchIndex) -> Vec<u8> {
        let mut buf = Vec::new();
        index.write_ed_to(&mut buf).unwrap();
        buf
    }

    #[test]
    fn builder_strips_overlap_and_derives_manifest() {
        let index = sample_index();
        assert_eq!(index.texts[1], "rust body content continues");
        assert_eq!(index.overlap_words, vec![0, 2, 0]);
        assert_eq!(index.manifest.chunks, 3);
        assert_eq!(index.manifest.pages, 2);
        assert_eq!(index.manifest.dense.len(), 1);
        assert_eq!(index.manifest.sparse.as_ref().unwrap().terms, 2);
        assert_eq!(index.manifest.sections, vec!["qa", "claims"]);
        assert_eq!(index.page_chunks("/a/"), vec![0, 1]);
        assert_eq!(index.page_chunks("/nope/"), Vec::<usize>::new());
    }

    #[test]
    fn round_trip_all_sections() {
        let index = sample_index();
        let bytes = ed_bytes(&index);
        let restored = SearchIndex::from_bytes(&bytes).unwrap();
        assert_eq!(restored.manifest, index.manifest);
        assert_eq!(restored.texts, index.texts);
        assert_eq!(restored.overlap_words, index.overlap_words);
        assert_eq!(restored.bm25, index.bm25);
        assert_eq!(restored.sparse, index.sparse);
        assert_eq!(restored.dense, index.dense);
        assert_eq!(restored.qa.len(), 1);
        assert_eq!(restored.qa_dense, index.qa_dense);
        assert_eq!(restored.claims.len(), 1);
        assert_eq!(restored.claims_dense, index.claims_dense);
        assert_eq!(restored.metadata.len(), 3);
        assert_eq!(restored.metadata[2].url, "/b/");

        let manifest = SearchIndex::manifest_from_bytes(&bytes).unwrap();
        assert_eq!(manifest, index.manifest);

        let info = SearchIndex::inspect(&bytes, Some(5)).unwrap();
        let names: Vec<&str> = info.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "meta",
                "texts",
                "bm25",
                "sparse",
                "dense/chunks/minilm",
                "qa",
                "dense/qa/minilm",
                "claims",
                "dense/claims/minilm"
            ]
        );
        assert_eq!(info.file_bytes, bytes.len());
    }

    #[test]
    fn output_is_deterministic() {
        let a = ed_bytes(&sample_index());
        let b = ed_bytes(&sample_index());
        assert_eq!(a, b);
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = ed_bytes(&sample_index());
        for cut in 0..bytes.len() {
            assert!(
                SearchIndex::from_bytes(&bytes[..cut]).is_err(),
                "cut at {}",
                cut
            );
        }
    }

    #[test]
    fn corruption_is_detected() {
        let index = sample_index();
        let bytes = ed_bytes(&index);
        // Flip a byte inside the compressed payload: either brotli fails or the CRC catches it.
        let mut bad = bytes.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert!(SearchIndex::from_bytes(&bad).is_err());

        // Wrong CRC in the header.
        let mut bad = bytes.clone();
        let manifest_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let crc_at = 12 + manifest_len + 4;
        bad[crc_at] ^= 0x01;
        let err = SearchIndex::from_bytes(&bad).unwrap_err().to_string();
        assert!(err.contains("CRC"), "{}", err);

        // Declared decompressed length too small / too large.
        let len_at = crc_at + 4;
        let mut bad = bytes.clone();
        bad[len_at..len_at + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(SearchIndex::from_bytes(&bad).is_err());
        let mut bad = bytes.clone();
        bad[len_at..len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SearchIndex::from_bytes(&bad).is_err());

        // Payload length larger than the file.
        let mut bad = bytes.clone();
        let plen_at = 12 + manifest_len;
        bad[plen_at..plen_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SearchIndex::from_bytes(&bad).is_err());
    }

    #[test]
    fn legacy_formats_are_rejected_with_a_hint() {
        let mut v1 = Vec::new();
        v1.extend_from_slice(b"SAED");
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&[0u8; 32]);
        let err = SearchIndex::from_bytes(&v1).unwrap_err().to_string();
        assert!(err.contains("eddie 0.4"), "{}", err);
        assert!(SearchIndex::manifest_from_bytes(&v1).is_err());

        let mut v4 = Vec::new();
        v4.extend_from_slice(b"SAGI");
        v4.extend_from_slice(&4u32.to_le_bytes());
        v4.extend_from_slice(&[0u8; 32]);
        let err = SearchIndex::from_bytes(&v4).unwrap_err().to_string();
        assert!(err.contains("eddie 0.4"), "{}", err);

        assert!(SearchIndex::from_bytes(b"").is_err());
        assert!(SearchIndex::from_bytes(b"XYZW").is_err());
    }

    #[test]
    fn unknown_sections_are_skipped_and_required_ones_enforced() {
        let index = sample_index();
        let mut payload = index.payload_bytes().unwrap();
        write_section(&mut payload, "future/thing", b"whatever").unwrap();
        write_section(&mut payload, "dense/other/minilm", &[0u8; 9]).unwrap();
        let restored = SearchIndex::from_payload(index.manifest.clone(), &payload).unwrap();
        assert_eq!(restored.texts, index.texts);

        // Drop the bm25 section: required.
        let mut stripped = Vec::new();
        stripped.extend_from_slice(PAYLOAD_MAGIC);
        stripped.extend_from_slice(&PAYLOAD_VERSION.to_le_bytes());
        for (name, body) in iter_sections(&payload).unwrap() {
            if name != SECTION_BM25 {
                write_section(&mut stripped, name, body).unwrap();
            }
        }
        let err = SearchIndex::from_payload(index.manifest.clone(), &stripped)
            .unwrap_err()
            .to_string();
        assert!(err.contains("bm25"), "{}", err);

        // A lane the manifest does not know.
        let mut extra = payload.clone();
        write_section(&mut extra, "dense/chunks/ghost", &[0u8; 9]).unwrap();
        assert!(SearchIndex::from_payload(index.manifest.clone(), &extra).is_err());
    }

    #[test]
    fn dims_times_rows_is_checked() {
        let spec = wasm_spec("x", 3, Quant::F32);
        let mut body = vec![0u8];
        body.extend_from_slice(&65536u32.to_le_bytes());
        body.extend_from_slice(&65536u32.to_le_bytes());
        // On 64-bit this asks for 16 GiB, on wasm32 it would wrap: both must error cleanly.
        assert!(DenseLane::from_bytes(spec.clone(), &body).is_err());

        let mut body = vec![1u8];
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(DenseLane::from_bytes(spec.clone(), &body).is_err());

        // Row count mismatch against chunks.
        let index = sample_index();
        let mut manifest = index.manifest.clone();
        manifest.chunks = 2;
        let payload = index.payload_bytes().unwrap();
        assert!(SearchIndex::from_payload(manifest, &payload).is_err());

        // NaN in f32 data.
        let mut body = vec![0u8];
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&f32::NAN.to_le_bytes());
        assert!(DenseLane::from_bytes(spec, &body).is_err());
    }

    #[test]
    fn dense_lane_int8_scores_match_f32_closely() {
        let dim = 8;
        let mut rng = Rng(42);
        let mut values: Vec<f32> = (0..dim * 50).map(|_| rng.normal()).collect();
        for row in values.chunks_exact_mut(dim) {
            normalize(row);
        }
        let f = DenseLane::from_f32(
            wasm_spec("f", dim, Quant::F32),
            dim,
            50,
            &values,
            Quant::F32,
        )
        .unwrap();
        let q = DenseLane::from_f32(
            wasm_spec("q", dim, Quant::Int8),
            dim,
            50,
            &values,
            Quant::Int8,
        )
        .unwrap();
        let mut query: Vec<f32> = (0..dim).map(|_| rng.normal()).collect();
        normalize(&mut query);
        let sf = f.scores(&query).unwrap();
        let sq = q.scores(&query).unwrap();
        for (a, b) in sf.iter().zip(&sq) {
            assert!((a - b).abs() < 0.02, "{} vs {}", a, b);
        }
        assert!(f.scores(&query[..dim - 1]).is_err());
        assert_eq!(q.row_f32(0).unwrap().len(), dim);
        assert!(q.row_f32(50).is_none());

        // Round trip of both quantisations.
        for lane in [&f, &q] {
            let back = DenseLane::from_bytes(lane.spec.clone(), &lane.to_bytes()).unwrap();
            assert_eq!(&back, lane);
            assert_eq!(lane.to_bytes().len(), lane.byte_len());
        }
    }

    #[test]
    fn top_k_selection_is_ordered_and_deterministic() {
        let out = select_top_k(vec![0.1, 0.9, 0.9, 0.5, 0.9], 3);
        assert_eq!(out, vec![(1, 0.9), (2, 0.9), (4, 0.9)]);
        let all = select_top_k(vec![0.1, 0.9], 10);
        assert_eq!(all, vec![(1, 0.9), (0, 0.1)]);
        assert!(select_top_k(vec![0.1], 0).is_empty());
    }

    #[test]
    fn int8_recall_agreement_on_synthetic_index() {
        let n = 2000;
        let dim = 384;
        let (index, corpus) = build_synthetic_index(n, dim, Quant::Int8, false);
        let f32_lane = DenseLane::from_f32(
            wasm_spec("f", dim, Quant::F32),
            dim,
            n,
            &corpus.vectors,
            Quant::F32,
        )
        .unwrap();
        let int8_lane = &index.dense[0];
        let mut rng = Rng(7);
        let queries = 100;
        let k = 10;
        let mut overlap_total = 0usize;
        for _ in 0..queries {
            let doc = rng.below(n);
            let mut q: Vec<f32> = corpus.vectors[doc * dim..(doc + 1) * dim]
                .iter()
                .map(|v| v + 0.3 * rng.normal())
                .collect();
            normalize(&mut q);
            let a: BTreeSet<usize> = f32_lane
                .top_k(&q, k)
                .unwrap()
                .into_iter()
                .map(|r| r.0)
                .collect();
            let b: BTreeSet<usize> = int8_lane
                .top_k(&q, k)
                .unwrap()
                .into_iter()
                .map(|r| r.0)
                .collect();
            overlap_total += a.intersection(&b).count();
        }
        let agreement = overlap_total as f64 / (queries * k) as f64;
        eprintln!("int8 vs f32 recall@10 agreement: {:.4}", agreement);
        assert!(agreement > 0.98, "agreement {}", agreement);
    }

    #[test]
    fn sparse_index_scoring_and_quantisation() {
        let index = sample_index();
        let sparse = index.sparse.as_ref().unwrap();
        assert_eq!(sparse.terms, vec![7, 9]);
        assert_eq!(sparse.idf_of(7), Some(2.5));
        assert_eq!(sparse.idf_of(9), Some(1.0)); // missing IDF -> neutral
        assert_eq!(sparse.idf_of(8), None);
        let hits = sparse.top_k(
            &[SparseTerm {
                token_id: 7,
                weight: 2.5,
            }],
            5,
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, 0);
        assert!((hits[0].1 - 2.5 * 1.2).abs() < 1e-3);
        assert!((hits[1].1 - 2.5 * 0.5).abs() < 1e-3);
        assert!(
            sparse
                .top_k(
                    &[SparseTerm {
                        token_id: 99,
                        weight: 1.0
                    }],
                    5
                )
                .is_empty()
        );
        assert!(sparse.top_k(&[], 5).is_empty());

        // Saturation and zero weights.
        let mut idf = HashMap::new();
        idf.insert(1u32, 1.0);
        let big = SparseIndex::build(
            &[vec![
                SparseTerm {
                    token_id: 1,
                    weight: 1000.0,
                },
                SparseTerm {
                    token_id: 2,
                    weight: 0.0001,
                },
                SparseTerm {
                    token_id: 3,
                    weight: -1.0,
                },
            ]],
            &idf,
        );
        assert_eq!(big.terms, vec![1]);
        assert_eq!(big.postings_for(1), vec![(0, 65.535)]);

        // Binary round trip and validation.
        let bytes = sparse.to_bytes().unwrap();
        assert_eq!(&SparseIndex::from_bytes(&bytes, 3).unwrap(), sparse);
        assert!(SparseIndex::from_bytes(&bytes, 1).is_err());
        for cut in 0..bytes.len() {
            assert!(SparseIndex::from_bytes(&bytes[..cut], 3).is_err());
        }
    }

    #[test]
    fn builder_rejects_inconsistent_input() {
        let mut b = IndexBuilder::new();
        assert!(
            b.add_chunks(vec![meta("/a/", 0, "fine")], vec![], vec![])
                .is_err()
        );
        b.add_chunks(vec![meta("/a/", 0, "fine")], vec!["x".into()], vec![0])
            .unwrap();
        assert!(
            b.add_dense_lane(
                "bogus",
                DenseLane::from_f32(wasm_spec("m", 2, Quant::F32), 2, 1, &[1.0, 0.0], Quant::F32)
                    .unwrap()
            )
            .is_err()
        );
        assert!(
            b.add_dense_lane(
                SCOPE_CHUNKS,
                DenseLane::from_f32(
                    wasm_spec("a/b", 2, Quant::F32),
                    2,
                    1,
                    &[1.0, 0.0],
                    Quant::F32
                )
                .unwrap()
            )
            .is_err()
        );
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(
                wasm_spec("m", 2, Quant::F32),
                2,
                2,
                &[1.0, 0.0, 0.0, 1.0],
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(b.finish().is_err(), "rows != chunks");

        assert!(
            DenseLane::from_f32(wasm_spec("m", 2, Quant::F32), 2, 1, &[1.0], Quant::F32).is_err()
        );
        assert!(
            DenseLane::from_f32(
                wasm_spec("m", 2, Quant::F32),
                2,
                1,
                &[f32::NAN, 0.0],
                Quant::F32
            )
            .is_err()
        );
    }

    #[test]
    fn strip_leading_words_cases() {
        assert_eq!(strip_leading_words("a b c", 0), "a b c");
        assert_eq!(strip_leading_words("a b c", 1), "b c");
        assert_eq!(strip_leading_words("  a   b c", 2), "c");
        assert_eq!(strip_leading_words("a b", 2), "");
        assert_eq!(strip_leading_words("a b", 5), "");
        assert_eq!(strip_leading_words("héllo wörld", 1), "wörld");
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// Size and latency report. Run with:
    /// `cargo test --release size_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn size_report() {
        use std::time::Instant;
        let n = 2000;
        let dim = 384;
        let (index, corpus) = build_synthetic_index(n, dim, Quant::Int8, true);
        let bytes = ed_bytes(&index);
        let info = SearchIndex::inspect(&bytes, Some(BROTLI_QUALITY)).unwrap();
        println!(
            "v5 index: {} bytes total ({} compressed payload, {} raw)",
            info.file_bytes, info.payload_compressed_bytes, info.payload_bytes
        );
        for s in &info.sections {
            println!(
                "  v5 {:<22} raw {:>9}  brotli {:>9}",
                s.name,
                s.raw_bytes,
                s.compressed_bytes.unwrap()
            );
        }

        // v4-equivalent layout: metadata JSON, f32 embeddings, JSON bm25 postings, texts.
        let meta_json = serde_json::to_vec(&corpus.metadata).unwrap();
        let mut emb = Vec::with_capacity(n * dim * 4);
        for v in &corpus.vectors {
            emb.extend_from_slice(&v.to_le_bytes());
        }
        let refs: Vec<&str> = corpus.texts.iter().map(String::as_str).collect();
        let bm25 = Bm25Index::build(&refs);
        let mut postings = std::collections::BTreeMap::new();
        for (t, p) in bm25.terms.iter().zip(&bm25.postings) {
            postings.insert(t.clone(), p.clone());
        }
        let bm25_json = serde_json::to_vec(&serde_json::json!({
            "num_docs": bm25.num_docs,
            "avg_doc_len": bm25.avg_doc_len,
            "doc_lengths": bm25.doc_lengths,
            "postings": postings,
        }))
        .unwrap();
        let mut texts = Vec::new();
        for t in &corpus.texts {
            texts.extend_from_slice(&(t.len() as u32).to_le_bytes());
            texts.extend_from_slice(t.as_bytes());
        }
        for (name, body) in [
            ("meta", &meta_json),
            ("embeddings f32", &emb),
            ("bm25 json", &bm25_json),
            ("texts", &texts),
        ] {
            let c = brotli_compress(body, BROTLI_QUALITY).unwrap();
            println!(
                "  v4 {:<22} raw {:>9}  brotli {:>9}",
                name,
                body.len(),
                c.len()
            );
        }

        // Latency: hybrid retrieve on the 2k index.
        let mut rng = Rng(99);
        let mut queries = Vec::new();
        for _ in 0..50 {
            let doc = rng.below(n);
            let mut q: Vec<f32> = corpus.vectors[doc * dim..(doc + 1) * dim]
                .iter()
                .map(|v| v + 0.3 * rng.normal())
                .collect();
            normalize(&mut q);
            let cluster = doc % 40;
            let sparse: Vec<SparseTerm> = (0..20)
                .map(|k| SparseTerm {
                    token_id: 30_000 + (cluster * 20 + k) as u32,
                    weight: 1.5,
                })
                .collect();
            queries.push((
                format!("topic{} facts w{}", cluster, rng.below(3000)),
                q,
                sparse,
            ));
        }
        let start = Instant::now();
        for (text, q, sparse) in &queries {
            let query = crate::search::Query {
                text,
                dense: Some((0, q.clone())),
                sparse: Some(sparse.clone()),
                mode: crate::search::Mode::Hybrid,
                top_k: 8,
                weights: crate::search::Weights::default(),
            };
            let r = crate::search::retrieve(&index, &query).unwrap();
            let pages =
                crate::search::group_pages(&index, &r.ranked, &crate::search::query_terms(text), 8);
            assert!(!pages.is_empty());
        }
        let per = start.elapsed().as_secs_f64() * 1000.0 / queries.len() as f64;
        println!(
            "hybrid retrieve+group_pages on {} chunks: {:.3} ms/query",
            n, per
        );

        let start = Instant::now();
        let loaded = SearchIndex::from_bytes(&bytes).unwrap();
        println!(
            "from_bytes: {:.1} ms ({} chunks)",
            start.elapsed().as_secs_f64() * 1000.0,
            loaded.metadata.len()
        );
    }
}
