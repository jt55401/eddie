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
//! | `bm25` | `u32 num_docs, f32 avg_len, u32 doc_lengths[n], u32 terms`, per term `u16 len, bytes, u32 postings, (varint doc_delta, varint tf)*` (see [`crate::bm25`]) |
//! | `sparse` | `u32 terms`, per term `u32 token_id, f32 idf, u32 postings, (varint doc_delta, u16 weight×1000)*` |
//! | `sparse/vocab` | `u8 version=1, u8 flags (1 clean_text, 2 chinese chars, 4 strip accents, 8 lowercase), u32 unk_id, u32 cls_id, u32 sep_id (u32::MAX = none), u32 max_input_chars, u16 prefix_len, prefix, u16 added_count, per added `u32 id, u8 special, u16 len, bytes`, u32 vocab_count, per id `u16 len, bytes`` (empty = unused id); lets the runtime WordPiece queries without `tokenizer.json` |
//! | `dense/<scope>/<lane_id>` | `u8 quant (0=f32, 1=int8), u32 dim, u32 rows`, rows×dim values, then for int8 `rows × f32 scale`; scope ∈ `chunks`, `qa`, `claims` |
//! | `qa` | JSON `Vec<QaEntry>` |
//! | `claims` | JSON `Vec<ClaimEntry>` |
//!
//! Every length is checked against the remaining bytes before any
//! allocation, dims × rows use checked arithmetic (wasm32 is 32-bit), and
//! bm25/sparse document ids are validated against the chunk count. Output is
//! deterministic: sections are written in a fixed order and every dictionary
//! is sorted.
//!
//! ## Sidecars
//!
//! [`SearchIndex::to_ed_split`] moves selected `dense/<scope>/<lane>`
//! sections out of the core file into one sidecar per lane
//! (`<stem>.<lane>.ed`): the same `SAED` container with its own CRC, a
//! payload holding only that lane's sections, and a manifest copy whose
//! `sidecar_lane` names the lane. The core manifest lists them in
//! `sidecars` and both carry the same `index_id`, so
//! [`SearchIndex::attach_sidecar`] can refuse a sidecar from another build.
//! A core index loads without its sidecars; searches that need an
//! unattached lane degrade the way a missing embedder does.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use brotli::{CompressorReader, Decompressor};
use sha2::{Digest, Sha256};

use crate::bm25::{Bm25Index, ByteCursor, write_varint};
use crate::manifest::{
    Bm25Params, DenseSpec, FORMAT_VERSION, FusionWeights, Manifest, Quant, SidecarSpec, SparseSpec,
    SparseTerm,
};
use crate::records::{ChunkMeta, ClaimEntry, QaEntry};
use crate::wordpiece::{AddedToken, Normalizer, WordPiece, WordPieceConfig};

const ED_MAGIC: &[u8; 4] = b"SAED";
const ED_VERSION: u32 = 2;
const PAYLOAD_MAGIC: &[u8; 4] = b"SAGI";
const PAYLOAD_VERSION: u32 = FORMAT_VERSION;

const SECTION_META: &str = "meta";
const SECTION_TEXTS: &str = "texts";
const SECTION_BM25: &str = "bm25";
const SECTION_SPARSE: &str = "sparse";
const SECTION_SPARSE_VOCAB: &str = "sparse/vocab";
const SPARSE_VOCAB_VERSION: u8 = 1;
const NO_ID: u32 = u32::MAX;
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
        if let Some(pos) = values.iter().position(|v| !v.is_finite()) {
            let bad_rows = values
                .chunks(dim.max(1))
                .filter(|row| row.iter().any(|v| !v.is_finite()))
                .count();
            bail!(
                "dense lane {:?}: non-finite value in row {} (of {} rows; {} rows affected)",
                spec.id,
                pos / dim.max(1),
                rows,
                bad_rows
            );
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
    /// L2-normalised). Errors when the query dimension does not match or
    /// the query is unusable (non-finite or all zeros, see
    /// [`query_vector_problem`]): such a vector would rank every row equal
    /// and turn the dense arm into index order.
    pub fn scores(&self, query: &[f32]) -> Result<Vec<f32>> {
        if query.len() != self.dim {
            bail!(
                "query has {} dims but lane {:?} has {}",
                query.len(),
                self.spec.id,
                self.dim
            );
        }
        if let Some(problem) = query_vector_problem(query) {
            bail!("query vector for lane {:?} {}", self.spec.id, problem);
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

/// Why a query vector cannot score a lane, if it cannot: it contains a NaN
/// or infinity, or every component is zero (nothing to rank by). `None`
/// means the vector is usable. Length is checked separately by the lane.
pub fn query_vector_problem(query: &[f32]) -> Option<&'static str> {
    if query.iter().any(|v| !v.is_finite()) {
        Some("contains non-finite values")
    } else if query.iter().all(|v| *v == 0.0) {
        Some("is all zeros")
    } else {
        None
    }
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
    /// to their maximum weight. Terms are visited in token-id order (postings
    /// are sorted by doc id), so every document's score is summed in the same
    /// order on every run and near-tie ranks are reproducible.
    pub fn top_k(&self, query: &[SparseTerm], k: usize) -> Vec<(usize, f32)> {
        if k == 0 || self.num_docs == 0 || query.is_empty() {
            return Vec::new();
        }
        let mut qmax: BTreeMap<u32, f32> = BTreeMap::new();
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
    /// The sparse query tokenizer, when the index embeds its vocabulary
    /// (`manifest.sparse.vocab == Embedded`).
    pub sparse_vocab: Option<WordPiece>,
    /// Chunk-scope dense lanes, in manifest order.
    pub dense: Vec<DenseLane>,
    pub qa: Vec<QaEntry>,
    pub qa_dense: Vec<DenseLane>,
    pub claims: Vec<ClaimEntry>,
    pub claims_dense: Vec<DenseLane>,
    /// BM25 over `question + " " + answer` of every QA entry, built on first
    /// use by [`SearchIndex::qa_bm25`] (never serialized).
    qa_bm25: OnceLock<Bm25Index>,
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

/// One sidecar file produced by [`SearchIndex::to_ed_split`].
#[derive(Debug, Clone)]
pub struct SidecarFile {
    /// File name relative to the core index.
    pub file: String,
    pub lane: String,
    /// Scopes whose `dense/<scope>/<lane>` section the file carries.
    pub scopes: Vec<String>,
    pub bytes: Vec<u8>,
}

/// A core index and its sidecar files, ready to be written.
#[derive(Debug, Clone)]
pub struct SplitIndex {
    pub core: Vec<u8>,
    pub sidecars: Vec<SidecarFile>,
}

/// What [`SearchIndex::attach_sidecar`] loaded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttachedSidecar {
    pub lane: String,
    pub scopes: Vec<String>,
}

impl SearchIndex {
    /// Serialize the index as one `SAED` v2 container with every section
    /// inline. Fails when a chunk lane the manifest lists is not loaded
    /// (attach every sidecar first).
    pub fn write_ed_to<W: Write>(&self, mut w: W) -> Result<()> {
        for spec in &self.manifest.dense {
            if !self.dense.iter().any(|l| l.spec.id == spec.id) {
                bail!(
                    "dense lane {:?} is not attached; attach its sidecar before writing a single-file index",
                    spec.id
                );
            }
        }
        let mut manifest = self.manifest.clone();
        manifest.sidecars.clear();
        manifest.sidecar_lane = None;
        manifest.index_id = Some(self.index_id()?);
        let payload = self.payload_with(&|_, _| true)?;
        w.write_all(&container_bytes(&manifest, &payload)?)?;
        Ok(())
    }

    /// Serialize as a core file plus one sidecar per lane for the
    /// `(scope, lane)` sections `select` returns `true` for. Sidecar files
    /// are named `<stem>.<lane>.ed`; the core manifest lists them in
    /// `sidecars` and shares its `index_id` with them.
    pub fn to_ed_split(
        &self,
        stem: &str,
        select: &dyn Fn(&str, &DenseSpec) -> bool,
    ) -> Result<SplitIndex> {
        let mut base = self.manifest.clone();
        base.sidecars.clear();
        base.sidecar_lane = None;
        base.index_id = Some(self.index_id()?);

        let mut sidecars = Vec::new();
        let mut entries = Vec::new();
        for spec in &self.manifest.dense {
            let scopes: Vec<String> = [
                (SCOPE_CHUNKS, &self.dense),
                (SCOPE_QA, &self.qa_dense),
                (SCOPE_CLAIMS, &self.claims_dense),
            ]
            .into_iter()
            .filter(|(scope, lanes)| {
                lanes.iter().any(|l| l.spec.id == spec.id) && select(scope, spec)
            })
            .map(|(scope, _)| scope.to_string())
            .collect();
            if scopes.is_empty() {
                continue;
            }
            let file = format!("{}.{}.ed", stem, spec.id);
            let payload = self.lane_payload(&spec.id, &scopes)?;
            let mut manifest = base.clone();
            manifest.sidecar_lane = Some(spec.id.clone());
            let bytes = container_bytes(&manifest, &payload)?;
            for scope in &scopes {
                entries.push(SidecarSpec {
                    file: file.clone(),
                    lane: spec.id.clone(),
                    scope: scope.clone(),
                    bytes: bytes.len() as u64,
                });
            }
            sidecars.push(SidecarFile {
                file,
                lane: spec.id.clone(),
                scopes,
                bytes,
            });
        }

        let mut core_manifest = base;
        core_manifest.sidecars = entries;
        let payload =
            self.payload_with(&|scope, lane| core_manifest.sidecar_for(scope, lane).is_none())?;
        let core = container_bytes(&core_manifest, &payload)?;
        Ok(SplitIndex { core, sidecars })
    }

    /// Identity shared by a core index and its sidecars: the first 16 hex
    /// digits of the SHA-256 of the chunk metadata, the texts and the dense
    /// lane specs. Two builds of the same content with the same lanes agree;
    /// anything else differs.
    pub fn index_id(&self) -> Result<String> {
        let mut h = Sha256::new();
        h.update(serde_json::to_vec(&self.metadata).context("serializing chunk metadata")?);
        for (text, overlap) in self.texts.iter().zip(&self.overlap_words) {
            h.update((text.len() as u64).to_le_bytes());
            h.update(text.as_bytes());
            h.update(overlap.to_le_bytes());
        }
        h.update(serde_json::to_vec(&self.manifest.dense).context("serializing dense specs")?);
        let digest = h.finalize();
        let mut out = String::with_capacity(16);
        for b in &digest[..8] {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        Ok(out)
    }

    /// Load a sidecar file's dense sections into this index. Checks that the
    /// file is a sidecar, that its `index_id` and chunk count match, and that
    /// every section matches the manifest lane (dim, quant, rows). Attaching
    /// the same sidecar twice replaces the sections; chunk lanes keep
    /// manifest order.
    pub fn attach_sidecar(&mut self, bytes: &[u8]) -> Result<AttachedSidecar> {
        let container = parse_container(bytes)?;
        let lane_id = container.manifest.sidecar_lane.clone().context(
            "not a sidecar file (its manifest has no sidecar_lane); sidecars are the <index>.<lane>.ed files written next to the core index",
        )?;
        match (&self.manifest.index_id, &container.manifest.index_id) {
            (Some(core), Some(side)) if core == side => {}
            (Some(core), Some(side)) => bail!(
                "sidecar for lane {:?} belongs to another index build (index_id {} but this index is {})",
                lane_id,
                side,
                core
            ),
            _ => bail!(
                "cannot verify the sidecar for lane {:?}: the core index or the sidecar carries no index_id",
                lane_id
            ),
        }
        if container.manifest.chunks != self.manifest.chunks {
            bail!(
                "sidecar for lane {:?} has {} chunks but this index has {}",
                lane_id,
                container.manifest.chunks,
                self.manifest.chunks
            );
        }
        let spec = self
            .manifest
            .dense_lane(&lane_id)
            .cloned()
            .with_context(|| format!("sidecar lane {:?} is not in the manifest", lane_id))?;
        let payload = brotli_decompress_exact(container.compressed, container.decompressed_len)
            .context("decompressing sidecar payload")?;
        if crc32(&payload) != container.crc32 {
            bail!("sidecar payload CRC mismatch; the file is corrupt");
        }

        let counts = [
            (SCOPE_CHUNKS, self.manifest.chunks, "chunks"),
            (SCOPE_QA, self.qa.len(), "qa entries"),
            (SCOPE_CLAIMS, self.claims.len(), "claims"),
        ];
        let mut scopes = Vec::new();
        for (name, body) in iter_sections(&payload)? {
            let Some((scope, lane)) = parse_lane_section_name(name) else {
                continue;
            };
            if lane != lane_id {
                bail!(
                    "sidecar for lane {:?} carries section {:?} of another lane",
                    lane_id,
                    name
                );
            }
            let Some(&(_, expected_rows, what)) = counts.iter().find(|(s, _, _)| *s == scope)
            else {
                continue;
            };
            let lane = DenseLane::from_bytes(spec.clone(), body)
                .with_context(|| format!("reading sidecar section {:?}", name))?;
            if lane.dim != spec.dim || lane.quant != spec.quant {
                bail!(
                    "sidecar section {:?} is {}-d {:?} but the manifest lane is {}-d {:?}",
                    name,
                    lane.dim,
                    lane.quant,
                    spec.dim,
                    spec.quant
                );
            }
            if lane.rows != expected_rows {
                bail!(
                    "sidecar section {:?} has {} rows but the index has {} {}",
                    name,
                    lane.rows,
                    expected_rows,
                    what
                );
            }
            let target = match scope {
                SCOPE_CHUNKS => &mut self.dense,
                SCOPE_QA => &mut self.qa_dense,
                _ => &mut self.claims_dense,
            };
            match target.iter().position(|l| l.spec.id == lane_id) {
                Some(pos) => target[pos] = lane,
                None => target.push(lane),
            }
            scopes.push(scope.to_string());
        }
        if scopes.is_empty() {
            bail!("sidecar for lane {:?} has no dense sections", lane_id);
        }
        let order = &self.manifest.dense;
        for lanes in [&mut self.dense, &mut self.qa_dense, &mut self.claims_dense] {
            lanes.sort_by_key(|l| order.iter().position(|s| s.id == l.spec.id));
        }
        Ok(AttachedSidecar {
            lane: lane_id,
            scopes,
        })
    }

    /// The uncompressed `SAGI` v5 payload with every section inline.
    pub fn payload_bytes(&self) -> Result<Vec<u8>> {
        self.payload_with(&|_, _| true)
    }

    /// The `SAGI` payload holding only `lane_id`'s sections for `scopes`.
    fn lane_payload(&self, lane_id: &str, scopes: &[String]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(PAYLOAD_MAGIC);
        out.extend_from_slice(&PAYLOAD_VERSION.to_le_bytes());
        for scope in scopes {
            let lanes = match scope.as_str() {
                SCOPE_CHUNKS => &self.dense,
                SCOPE_QA => &self.qa_dense,
                SCOPE_CLAIMS => &self.claims_dense,
                other => bail!("unknown dense scope {:?}", other),
            };
            let lane = lanes
                .iter()
                .find(|l| l.spec.id == lane_id)
                .with_context(|| format!("no dense/{}/{} lane to split out", scope, lane_id))?;
            write_section(
                &mut out,
                &lane_section_name(scope, lane_id),
                &lane.to_bytes(),
            )?;
        }
        Ok(out)
    }

    /// The `SAGI` payload with the dense sections `keep(scope, lane)`
    /// accepts (the rest go to sidecars).
    fn payload_with(&self, keep: &dyn Fn(&str, &str) -> bool) -> Result<Vec<u8>> {
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
            if let Some(vocab) = &self.sparse_vocab {
                write_section(
                    &mut out,
                    SECTION_SPARSE_VOCAB,
                    &sparse_vocab_to_bytes(vocab)?,
                )?;
            }
        }
        for lane in &self.dense {
            if keep(SCOPE_CHUNKS, &lane.spec.id) {
                write_section(
                    &mut out,
                    &lane_section_name(SCOPE_CHUNKS, &lane.spec.id),
                    &lane.to_bytes(),
                )?;
            }
        }
        if !self.qa.is_empty() {
            write_section(
                &mut out,
                SECTION_QA,
                &serde_json::to_vec(&self.qa).context("serializing qa entries")?,
            )?;
            for lane in &self.qa_dense {
                if keep(SCOPE_QA, &lane.spec.id) {
                    write_section(
                        &mut out,
                        &lane_section_name(SCOPE_QA, &lane.spec.id),
                        &lane.to_bytes(),
                    )?;
                }
            }
        }
        if !self.claims.is_empty() {
            write_section(
                &mut out,
                SECTION_CLAIMS,
                &serde_json::to_vec(&self.claims).context("serializing claims")?,
            )?;
            for lane in &self.claims_dense {
                if keep(SCOPE_CLAIMS, &lane.spec.id) {
                    write_section(
                        &mut out,
                        &lane_section_name(SCOPE_CLAIMS, &lane.spec.id),
                        &lane.to_bytes(),
                    )?;
                }
            }
        }
        Ok(out)
    }

    /// Parse only the uncompressed header of an `.ed` file.
    pub fn manifest_from_bytes(bytes: &[u8]) -> Result<Manifest> {
        Ok(parse_container(bytes)?.manifest)
    }

    /// Parse and validate a whole `.ed` file (a core index or a 0.4.1
    /// single-file index; sidecar files are rejected with a hint).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let container = parse_container(bytes)?;
        if let Some(lane) = &container.manifest.sidecar_lane {
            bail!(
                "this file is the sidecar for dense lane {:?}; load the core index first and attach it with attach_sidecar",
                lane
            );
        }
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
        let mut sparse_vocab: Option<WordPiece> = None;
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
                SECTION_SPARSE_VOCAB => {
                    sparse_vocab =
                        Some(sparse_vocab_from_bytes(body).context("reading sparse/vocab")?);
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
        match (&manifest.sparse, &sparse_vocab) {
            (Some(spec), None) if spec.vocab == crate::manifest::SparseVocab::Embedded => {
                bail!(
                    "manifest says the sparse vocab is embedded but the payload has no sparse/vocab section"
                )
            }
            (None, Some(_)) => bail!("payload has a sparse/vocab section but no sparse arm"),
            _ => {}
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
        // Keep chunk lanes in manifest order and require one section per
        // lane unless the manifest says the section lives in a sidecar.
        dense.sort_by_key(|l| manifest.dense.iter().position(|s| s.id == l.spec.id));
        for spec in &manifest.dense {
            if !dense.iter().any(|l| l.spec.id == spec.id)
                && manifest.sidecar_for(SCOPE_CHUNKS, &spec.id).is_none()
            {
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
            sparse_vocab,
            dense,
            qa,
            qa_dense,
            claims,
            claims_dense,
            qa_bm25: OnceLock::new(),
        })
    }

    /// Chunk ids for a page URL in document order: finest granularity first
    /// (see [`granularity_rank`]), then `chunk_index`, then id. Each
    /// granularity pass restarts `chunk_index` at 0, so sorting by
    /// `chunk_index` alone would interleave fine, coarse and summary text.
    pub fn page_chunks(&self, url: &str) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .metadata
            .iter()
            .enumerate()
            .filter(|(_, m)| m.url == url)
            .map(|(i, _)| i)
            .collect();
        ids.sort_by_key(|&i| {
            let m = &self.metadata[i];
            (granularity_rank(m.granularity.as_deref()), m.chunk_index, i)
        });
        ids
    }

    /// The page's chunks at its finest granularity only (`fine` when
    /// present, else whatever the page has), in `chunk_index` order: the
    /// page text exactly once, without coarse or summary duplicates.
    pub fn page_chunks_finest(&self, url: &str) -> Vec<usize> {
        let ids = self.page_chunks(url);
        let Some(&first) = ids.first() else {
            return ids;
        };
        let finest = granularity_rank(self.metadata[first].granularity.as_deref());
        ids.into_iter()
            .take_while(|&i| granularity_rank(self.metadata[i].granularity.as_deref()) == finest)
            .collect()
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

    /// Keyword index over the QA entries (`question + " " + answer`), built
    /// on the first call and cached. Cheap: QA sections are a few hundred
    /// short strings. Empty (zero documents) when the index has no qa section.
    pub fn qa_bm25(&self) -> &Bm25Index {
        self.qa_bm25.get_or_init(|| {
            let texts: Vec<String> = self
                .qa
                .iter()
                .map(|e| format!("{} {}", e.question, e.answer))
                .collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            Bm25Index::build(&refs)
        })
    }
}

/// Page context prepended to a chunk's *indexed* text (dense, sparse and
/// BM25 inputs) so a query that names the page finds a chunk whose body never
/// repeats the title: `"{title}"`, plus `" — {section}"` when the chunk has a
/// section that differs from the title. Empty when the chunk has neither.
pub fn context_prefix(meta: &ChunkMeta) -> String {
    let title = meta.title.trim();
    let section = meta
        .section
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(title));
    match (title.is_empty(), section) {
        (true, None) => String::new(),
        (true, Some(section)) => section.to_string(),
        (false, None) => title.to_string(),
        (false, Some(section)) => format!("{} — {}", title, section),
    }
}

/// `prefix + "\n" + text`, or `text` alone when the prefix is empty.
pub fn with_context(prefix: &str, text: &str) -> String {
    if prefix.is_empty() {
        text.to_string()
    } else {
        format!("{}\n{}", prefix, text)
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
    /// What BM25 is built from when it differs from the stored texts
    /// (see [`IndexBuilder::add_chunks_indexed`]).
    index_texts: Option<Vec<String>>,
    title_context: bool,
    fusion: Option<FusionWeights>,
    overlap_words: Vec<u16>,
    bm25_params: Bm25Params,
    sparse: Option<(SparseIndex, SparseSpec)>,
    sparse_vocab: Option<WordPiece>,
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
        self.index_texts = None;
        self.overlap_words = overlap_words;
        Ok(self)
    }

    /// Like [`IndexBuilder::add_chunks`], but BM25 is built from
    /// `index_texts[i]` instead of the stored text: typically the clean text
    /// with a [`context_prefix`] in front. `stored_texts` are still stripped
    /// of their overlap prefix and are what display and snippets use;
    /// `index_texts` are consumed here and never written to the file.
    pub fn add_chunks_indexed(
        &mut self,
        metadata: Vec<ChunkMeta>,
        stored_texts: Vec<String>,
        index_texts: Vec<String>,
        overlap_words: Vec<u16>,
    ) -> Result<&mut Self> {
        if index_texts.len() != metadata.len() {
            bail!(
                "add_chunks_indexed: {} metadata but {} index texts",
                metadata.len(),
                index_texts.len()
            );
        }
        self.add_chunks(metadata, stored_texts, overlap_words)?;
        self.index_texts = Some(index_texts);
        Ok(self)
    }

    /// Record in the manifest that the indexed texts carried a
    /// [`context_prefix`] (so `eddie stats` and `eddie search --explain` can
    /// say so). Does not change what is indexed; pair it with
    /// [`IndexBuilder::add_chunks_indexed`].
    pub fn title_context(&mut self, enabled: bool) -> &mut Self {
        self.title_context = enabled;
        self
    }

    /// Bake fusion weights into the manifest (`eddie index --weights`).
    pub fn fusion(&mut self, weights: Option<FusionWeights>) -> &mut Self {
        self.fusion = weights;
        self
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

    /// Embed the sparse query tokenizer's vocabulary (`sparse/vocab`
    /// section) so the runtime needs no `tokenizer.json`; sets the manifest's
    /// `sparse.vocab` to `embedded`. Needs [`IndexBuilder::add_sparse`].
    pub fn sparse_vocab(&mut self, tokenizer: WordPiece) -> &mut Self {
        self.sparse_vocab = Some(tokenizer);
        self
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
        let bm25_source = self.index_texts.as_ref().unwrap_or(&self.texts);
        let refs: Vec<&str> = bm25_source.iter().map(String::as_str).collect();
        let bm25 = Bm25Index::build_with_params(&refs, self.bm25_params);

        for lane in &self.dense {
            check_rows(SCOPE_CHUNKS, lane, chunks)?;
        }
        for lane in &self.qa_dense {
            check_rows(SCOPE_QA, lane, self.qa.len())?;
            check_matches_chunk_lane(SCOPE_QA, lane, &self.dense)?;
        }
        for lane in &self.claims_dense {
            check_rows(SCOPE_CLAIMS, lane, self.claims.len())?;
            check_matches_chunk_lane(SCOPE_CLAIMS, lane, &self.dense)?;
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
        manifest.title_context = self.title_context;
        manifest.fusion = self.fusion;
        let (sparse, mut sparse_spec) = match self.sparse {
            Some((idx, spec)) => (Some(idx), Some(spec)),
            None => (None, None),
        };
        let sparse_vocab = match (&mut sparse_spec, self.sparse_vocab) {
            (Some(spec), Some(vocab)) => {
                spec.vocab = crate::manifest::SparseVocab::Embedded;
                Some(vocab)
            }
            (None, Some(_)) => bail!("sparse_vocab needs a sparse arm (call add_sparse first)"),
            (Some(spec), None) => {
                spec.vocab = crate::manifest::SparseVocab::Fetch;
                None
            }
            (None, None) => None,
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
            sparse_vocab,
            dense: self.dense,
            qa: self.qa,
            qa_dense: self.qa_dense,
            claims: self.claims,
            claims_dense: self.claims_dense,
            qa_bm25: OnceLock::new(),
        };
        // Lanes for empty sections are dropped (nothing to score).
        if index.qa.is_empty() {
            index.qa_dense.clear();
        }
        if index.claims.is_empty() {
            index.claims_dense.clear();
        }
        index.manifest.index_id = Some(index.index_id()?);
        Ok(index)
    }
}

/// Sort key for chunk granularities, finest first: `fine`, then unlabelled,
/// then `coarse`, then `summary`, then anything else.
pub fn granularity_rank(granularity: Option<&str>) -> u8 {
    match granularity {
        Some("fine") => 0,
        None => 1,
        Some("coarse") => 2,
        Some("summary") => 3,
        Some(_) => 4,
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

/// A qa/claims lane is described by the manifest entry of the chunks lane
/// with the same id, so its dim and quant must match or the reader would
/// reject the file the builder just wrote.
fn check_matches_chunk_lane(scope: &str, lane: &DenseLane, chunks: &[DenseLane]) -> Result<()> {
    let Some(chunk_lane) = chunks.iter().find(|l| l.spec.id == lane.spec.id) else {
        bail!(
            "{} lane {:?} has no matching chunks lane",
            scope,
            lane.spec.id
        );
    };
    if lane.dim != chunk_lane.dim {
        bail!(
            "dense/{}/{} has dim {} but the chunks lane has dim {}",
            scope,
            lane.spec.id,
            lane.dim,
            chunk_lane.dim
        );
    }
    if lane.quant != chunk_lane.quant {
        bail!(
            "dense/{}/{} is {:?} but the chunks lane is {:?}",
            scope,
            lane.spec.id,
            lane.quant,
            chunk_lane.quant
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

/// `sparse/vocab` section body (see the module docs).
fn sparse_vocab_to_bytes(tokenizer: &WordPiece) -> Result<Vec<u8>> {
    let (tokens, config) = tokenizer.to_vocab();
    let n = &config.normalizer;
    let flags = (n.clean_text as u8)
        | ((n.handle_chinese_chars as u8) << 1)
        | ((n.strip_accents as u8) << 2)
        | ((n.lowercase as u8) << 3);
    let mut out = Vec::with_capacity(tokens.len() * 8);
    out.push(SPARSE_VOCAB_VERSION);
    out.push(flags);
    out.extend_from_slice(&config.unk_id.to_le_bytes());
    out.extend_from_slice(&config.cls_id.unwrap_or(NO_ID).to_le_bytes());
    out.extend_from_slice(&config.sep_id.unwrap_or(NO_ID).to_le_bytes());
    out.extend_from_slice(&len_u32(config.max_input_chars, "max_input_chars")?.to_le_bytes());
    out.extend_from_slice(&len_u16(config.prefix.len(), "vocab prefix")?.to_le_bytes());
    out.extend_from_slice(config.prefix.as_bytes());
    out.extend_from_slice(&len_u16(config.added.len(), "added tokens")?.to_le_bytes());
    for tok in &config.added {
        out.extend_from_slice(&tok.id.to_le_bytes());
        out.push(tok.special as u8);
        out.extend_from_slice(&len_u16(tok.content.len(), "added token")?.to_le_bytes());
        out.extend_from_slice(tok.content.as_bytes());
    }
    out.extend_from_slice(&len_u32(tokens.len(), "vocab")?.to_le_bytes());
    for token in &tokens {
        out.extend_from_slice(&len_u16(token.len(), "vocab token")?.to_le_bytes());
        out.extend_from_slice(token.as_bytes());
    }
    Ok(out)
}

fn sparse_vocab_from_bytes(body: &[u8]) -> Result<WordPiece> {
    let mut c = ByteCursor::new(body);
    let version = c.u8().context("vocab version")?;
    if version != SPARSE_VOCAB_VERSION {
        bail!("unsupported sparse/vocab version {}", version);
    }
    let flags = c.u8().context("vocab flags")?;
    let normalizer = Normalizer {
        clean_text: flags & 1 != 0,
        handle_chinese_chars: flags & 2 != 0,
        strip_accents: flags & 4 != 0,
        lowercase: flags & 8 != 0,
    };
    let unk_id = c.u32().context("unk id")?;
    let opt = |v: u32| (v != NO_ID).then_some(v);
    let cls_id = opt(c.u32().context("cls id")?);
    let sep_id = opt(c.u32().context("sep id")?);
    let max_input_chars = c.u32().context("max_input_chars")? as usize;
    let prefix_len = c.u16().context("prefix length")? as usize;
    let prefix = std::str::from_utf8(c.bytes(prefix_len).context("prefix")?)
        .context("prefix is not UTF-8")?
        .to_string();
    let added_count = c.u16().context("added token count")? as usize;
    let mut added = Vec::with_capacity(added_count);
    for i in 0..added_count {
        let id = c.u32().with_context(|| format!("added token {} id", i))?;
        let special = c.u8().with_context(|| format!("added token {} flag", i))? != 0;
        let len = c
            .u16()
            .with_context(|| format!("added token {} length", i))? as usize;
        let content = std::str::from_utf8(c.bytes(len).context("added token")?)
            .with_context(|| format!("added token {} is not UTF-8", i))?
            .to_string();
        added.push(AddedToken {
            content,
            id,
            special,
        });
    }
    let count = c.u32().context("vocab count")? as usize;
    // Every entry costs at least its 2-byte length.
    let mut tokens = Vec::with_capacity(count.min(c.remaining() / 2));
    for i in 0..count {
        let len = c
            .u16()
            .with_context(|| format!("vocab token {} length", i))? as usize;
        let token = std::str::from_utf8(c.bytes(len).context("vocab token")?)
            .with_context(|| format!("vocab token {} is not UTF-8", i))?;
        tokens.push(token.to_string());
    }
    if c.remaining() != 0 {
        bail!("sparse/vocab section has {} trailing bytes", c.remaining());
    }
    WordPiece::from_vocab(
        tokens,
        WordPieceConfig {
            normalizer,
            unk_id,
            cls_id,
            sep_id,
            max_input_chars,
            prefix,
            added,
            max_tokens: crate::sparse::DEFAULT_MAX_SEQ_LEN.saturating_sub(2),
        },
    )
}

fn len_u16(len: usize, what: &str) -> Result<u16> {
    u16::try_from(len).with_context(|| format!("{} exceeds 64 KiB", what))
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

/// A `SAED` v2 container around `payload` (brotli-compressed, CRC-32 of the
/// uncompressed bytes in the header).
fn container_bytes(manifest: &Manifest, payload: &[u8]) -> Result<Vec<u8>> {
    let manifest_json = serde_json::to_vec(manifest).context("serializing manifest")?;
    let crc = crc32(payload);
    let compressed = brotli_compress(payload, BROTLI_QUALITY).context("compressing payload")?;
    let mut out = Vec::with_capacity(24 + manifest_json.len() + compressed.len());
    out.extend_from_slice(ED_MAGIC);
    out.extend_from_slice(&ED_VERSION.to_le_bytes());
    out.extend_from_slice(&len_u32(manifest_json.len(), "manifest")?.to_le_bytes());
    out.extend_from_slice(&manifest_json);
    out.extend_from_slice(&len_u32(compressed.len(), "compressed payload")?.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&len_u32(payload.len(), "payload")?.to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
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
                base_url: None,
                bytes: None,
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
                        vocab: crate::manifest::SparseVocab::Fetch,
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
    use crate::manifest::{Family, RuntimeSpec};

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
                vocab: crate::manifest::SparseVocab::Fetch,
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
    fn context_prefix_joins_title_and_distinct_section() {
        let mut m = meta("/a/", 0, "fine");
        m.title = "Programming Languages".into();
        m.section = None;
        assert_eq!(context_prefix(&m), "Programming Languages");
        m.section = Some("Rust".into());
        assert_eq!(context_prefix(&m), "Programming Languages — Rust");
        // Section equal to the title (any case, padded) is not repeated.
        m.section = Some(" programming languages ".into());
        assert_eq!(context_prefix(&m), "Programming Languages");
        m.section = Some("   ".into());
        assert_eq!(context_prefix(&m), "Programming Languages");
        m.title = "  ".into();
        m.section = Some("Rust".into());
        assert_eq!(context_prefix(&m), "Rust");
        m.section = None;
        assert_eq!(context_prefix(&m), "");
        assert_eq!(with_context("", "body"), "body");
        assert_eq!(with_context("T — S", "body"), "T — S\nbody");
    }

    #[test]
    fn indexed_texts_feed_bm25_but_not_storage() {
        let metadata = vec![meta("/a/", 0, "fine"), meta("/b/", 0, "fine")];
        let stored = vec![
            "I've been coding since age 6.".to_string(),
            "Unrelated body text.".to_string(),
        ];
        let indexed: Vec<String> = metadata
            .iter()
            .zip(&stored)
            .map(|(m, t)| with_context(&context_prefix(m), t))
            .collect();
        let mut b = IndexBuilder::new();
        assert!(
            b.add_chunks_indexed(metadata.clone(), stored.clone(), vec![], vec![0, 0])
                .is_err()
        );
        b.add_chunks_indexed(metadata.clone(), stored.clone(), indexed, vec![0, 0])
            .unwrap();
        b.title_context(true);
        let index = b.finish().unwrap();
        assert!(index.manifest.title_context);
        // Stored text is clean; BM25 knows the title ("Title /a/") and section.
        assert_eq!(index.texts, stored);
        assert_eq!(index.bm25.search("title", 10).len(), 2);
        assert!(index.bm25.postings_for("s0").is_some());
        // Round trip keeps the flag; BM25 bytes carry the prefixed terms.
        let restored = SearchIndex::from_bytes(&ed_bytes(&index)).unwrap();
        assert!(restored.manifest.title_context);
        assert_eq!(restored.bm25, index.bm25);
        assert_eq!(restored.texts, stored);
        // Plain add_chunks after add_chunks_indexed forgets the index texts.
        let mut b = IndexBuilder::new();
        b.add_chunks_indexed(
            metadata.clone(),
            stored.clone(),
            vec!["zzz".into(), "zzz".into()],
            vec![0, 0],
        )
        .unwrap();
        b.add_chunks(metadata, stored, vec![0, 0]).unwrap();
        let index = b.finish().unwrap();
        assert!(index.bm25.postings_for("zzz").is_none());
        assert!(!index.manifest.title_context);
    }

    #[test]
    fn qa_bm25_is_lazy_and_covers_question_and_answer() {
        let index = sample_index();
        assert!(!index.qa.is_empty());
        let bm25 = index.qa_bm25();
        assert_eq!(bm25.num_docs, index.qa.len());
        // sample_index's entry is "Who?" / "Them.": both sides are indexed.
        assert_eq!(
            bm25.search("them", 5),
            vec![(0, bm25.search("them", 5)[0].1)]
        );
        assert!(!bm25.search("who", 5).is_empty());
        assert!(std::ptr::eq(bm25, index.qa_bm25()));
        let (empty, _) = testutil::build_synthetic_index(4, 4, Quant::F32, false);
        assert_eq!(empty.qa_bm25().num_docs, 0);
        assert!(empty.qa_bm25().search("anything", 3).is_empty());
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
    fn unusable_query_vectors_are_rejected() {
        let lane = DenseLane::from_f32(
            wasm_spec("m", 3, Quant::F32),
            3,
            2,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            Quant::F32,
        )
        .unwrap();
        assert_eq!(
            query_vector_problem(&[f32::NAN, 0.0, 0.0]),
            Some("contains non-finite values")
        );
        assert_eq!(
            query_vector_problem(&[f32::INFINITY, 1.0]),
            Some("contains non-finite values")
        );
        assert_eq!(
            query_vector_problem(&[0.0, 0.0, -0.0]),
            Some("is all zeros")
        );
        assert_eq!(query_vector_problem(&[0.0, 0.5, 0.0]), None);
        let err = lane
            .top_k(&[f32::NAN, 0.0, 0.0], 2)
            .unwrap_err()
            .to_string();
        assert!(err.contains("non-finite"), "{}", err);
        let err = lane.top_k(&[0.0; 3], 2).unwrap_err().to_string();
        assert!(err.contains("all zeros"), "{}", err);
        assert_eq!(lane.top_k(&[1.0, 0.0, 0.0], 1).unwrap(), vec![(0, 1.0)]);
    }

    #[test]
    fn builder_rejects_qa_and_claims_lanes_that_differ_from_the_chunks_lane() {
        let entry = QaEntry {
            question: "Who?".into(),
            answer: "Them.".into(),
            source_title: "A".into(),
            source_url: "/a/".into(),
            source_section: None,
            tags: vec![],
            confidence: 0.9,
        };
        let build = |qa_dim: usize, qa_quant: Quant| -> Result<SearchIndex> {
            let mut b = IndexBuilder::new();
            b.add_chunks(vec![meta("/a/", 0, "fine")], vec!["x".into()], vec![0])?;
            b.add_dense_lane(
                SCOPE_CHUNKS,
                DenseLane::from_f32(
                    wasm_spec("m", 3, Quant::Int8),
                    3,
                    1,
                    &[1.0, 0.0, 0.0],
                    Quant::Int8,
                )?,
            )?;
            b.add_qa(vec![entry.clone()]);
            let values = vec![1.0; qa_dim];
            b.add_dense_lane(
                SCOPE_QA,
                DenseLane::from_f32(
                    wasm_spec("m", qa_dim, qa_quant),
                    qa_dim,
                    1,
                    &values,
                    qa_quant,
                )?,
            )?;
            b.finish()
        };
        let err = build(2, Quant::Int8).unwrap_err().to_string();
        assert!(err.contains("dim 2") && err.contains("dim 3"), "{}", err);
        let err = build(3, Quant::F32).unwrap_err().to_string();
        assert!(err.contains("F32") && err.contains("Int8"), "{}", err);
        // A matching lane still round-trips through the reader.
        let index = build(3, Quant::Int8).unwrap();
        let restored = SearchIndex::from_bytes(&ed_bytes(&index)).unwrap();
        assert_eq!(restored.qa_dense, index.qa_dense);
    }

    #[test]
    fn page_chunks_orders_by_granularity_then_chunk_index() {
        // Fine 0..3, coarse 0..1 and a summary, all on one page, stored
        // interleaved so id order proves nothing.
        let metadata = vec![
            meta("/p/", 0, "coarse"),
            meta("/p/", 0, "fine"),
            meta("/p/", 0, "summary"),
            meta("/p/", 1, "fine"),
            meta("/p/", 1, "coarse"),
            meta("/p/", 2, "fine"),
            meta("/q/", 0, "coarse"),
        ];
        let n = metadata.len();
        let texts: Vec<String> = (0..n).map(|i| format!("text {}", i)).collect();
        let mut b = IndexBuilder::new();
        b.add_chunks(metadata, texts, vec![0; n]).unwrap();
        let index = b.finish().unwrap();
        assert_eq!(index.page_chunks("/p/"), vec![1, 3, 5, 0, 4, 2]);
        assert_eq!(index.page_chunks_finest("/p/"), vec![1, 3, 5]);
        // A page with only coarse chunks falls back to them.
        assert_eq!(index.page_chunks_finest("/q/"), vec![6]);
        assert!(index.page_chunks_finest("/nope/").is_empty());
        assert!(granularity_rank(Some("fine")) < granularity_rank(None));
        assert!(granularity_rank(None) < granularity_rank(Some("coarse")));
        assert!(granularity_rank(Some("coarse")) < granularity_rank(Some("summary")));
        assert!(granularity_rank(Some("summary")) < granularity_rank(Some("other")));
    }

    #[test]
    fn sparse_top_k_is_independent_of_query_term_order() {
        let corpus = synthetic_corpus(256, 4, 9);
        let sparse = SparseIndex::build(&corpus.sparse_docs, &corpus.idf);
        let mut rng = Rng(3);
        for _ in 0..20 {
            let mut query: Vec<SparseTerm> = (0..30)
                .map(|_| SparseTerm {
                    token_id: 30_000 + rng.below(800) as u32,
                    weight: 0.5 + rng.unit(),
                })
                .collect();
            let forward = sparse.top_k(&query, 20);
            query.reverse();
            let backward = sparse.top_k(&query, 20);
            assert_eq!(forward, backward);
            // Scores are bit-identical, not merely close.
            for (a, b) in forward.iter().zip(&backward) {
                assert_eq!(a.1.to_bits(), b.1.to_bits());
            }
        }
    }

    fn webgpu_spec(id: &str, dim: usize) -> DenseSpec {
        let mut spec = wasm_spec(id, dim, Quant::F32);
        spec.model = "Qwen/Qwen3-Embedding-0.6B".into();
        spec.family = Family::Qwen3;
        spec.runtime = RuntimeSpec::WebgpuOnnx {
            repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX".into(),
            dtype: "q4".into(),
            dtype_f16: None,
            pooling: "last_token".into(),
            base_url: None,
        };
        spec
    }

    /// `sample_index` plus a second (webgpu) chunk lane.
    fn two_lane_index() -> SearchIndex {
        let base = sample_index();
        let mut b = IndexBuilder::new();
        b.add_chunks(
            base.metadata.clone(),
            base.texts.clone(),
            base.overlap_words.clone(),
        )
        .unwrap();
        for lane in &base.dense {
            b.add_dense_lane(SCOPE_CHUNKS, lane.clone()).unwrap();
        }
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(
                webgpu_spec("qwen3e", 2),
                2,
                3,
                &[1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                Quant::F32,
            )
            .unwrap(),
        )
        .unwrap();
        b.add_qa(base.qa.clone());
        for lane in &base.qa_dense {
            b.add_dense_lane(SCOPE_QA, lane.clone()).unwrap();
        }
        b.add_dense_lane(
            SCOPE_QA,
            DenseLane::from_f32(webgpu_spec("qwen3e", 2), 2, 1, &[0.0, 1.0], Quant::F32).unwrap(),
        )
        .unwrap();
        b.add_claims(base.claims.clone());
        for lane in &base.claims_dense {
            b.add_dense_lane(SCOPE_CLAIMS, lane.clone()).unwrap();
        }
        b.finish().unwrap()
    }

    /// The default CLI policy: webgpu chunk lanes and every qa/claims lane
    /// go to sidecars, wasm-candle chunk lanes stay in the core.
    fn default_split(scope: &str, spec: &DenseSpec) -> bool {
        scope != SCOPE_CHUNKS || matches!(spec.runtime, RuntimeSpec::WebgpuOnnx { .. })
    }

    #[test]
    fn split_writes_one_sidecar_per_lane_and_attach_restores_them() {
        let index = two_lane_index();
        let split = index.to_ed_split("site", &default_split).unwrap();
        let names: Vec<&str> = split.sidecars.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(names, vec!["site.minilm.ed", "site.qwen3e.ed"]);
        assert_eq!(split.sidecars[0].scopes, vec!["qa", "claims"]);
        assert_eq!(split.sidecars[1].scopes, vec!["chunks", "qa"]);

        let core = SearchIndex::from_bytes(&split.core).unwrap();
        let m = &core.manifest;
        assert_eq!(m.dense.len(), 2, "manifest still lists both lanes");
        assert_eq!(core.dense.len(), 1, "only the wasm lane is inline");
        assert_eq!(core.dense[0].spec.id, "minilm");
        assert!(core.qa_dense.is_empty() && core.claims_dense.is_empty());
        assert_eq!(core.qa.len(), 1, "qa entries stay in the core");
        assert_eq!(m.sidecars.len(), 4);
        for s in &m.sidecars {
            let file = split.sidecars.iter().find(|f| f.file == s.file).unwrap();
            assert_eq!(s.bytes as usize, file.bytes.len(), "{}", s.file);
        }
        assert_eq!(
            m.sidecar_for("chunks", "qwen3e").unwrap().file,
            "site.qwen3e.ed"
        );
        assert!(m.sidecar_for("chunks", "minilm").is_none());
        assert_eq!(m.sidecar_files(), vec!["site.minilm.ed", "site.qwen3e.ed"]);
        assert_eq!(
            m.index_id.as_deref(),
            Some(index.index_id().unwrap().as_str())
        );
        assert!(m.sidecar_lane.is_none());
        // The core still searches with what it has.
        assert!(core.dense_lane("qwen3e").is_none());
        assert_eq!(core.dense_lane("minilm"), Some(0));

        // A sidecar is not an index.
        let err = SearchIndex::from_bytes(&split.sidecars[1].bytes).unwrap_err();
        assert!(
            err.to_string()
                .contains("sidecar for dense lane \"qwen3e\"")
        );
        // The sidecar's header is readable on its own.
        let side = SearchIndex::manifest_from_bytes(&split.sidecars[1].bytes).unwrap();
        assert_eq!(side.sidecar_lane.as_deref(), Some("qwen3e"));
        assert_eq!(side.index_id, m.index_id);
        assert!(side.sidecars.is_empty());

        // Attach in the "wrong" order: chunk lanes end up in manifest order.
        let mut core = core;
        let att = core.attach_sidecar(&split.sidecars[1].bytes).unwrap();
        assert_eq!(att.lane, "qwen3e");
        assert_eq!(att.scopes, vec!["chunks", "qa"]);
        assert_eq!(core.dense_lane("minilm"), Some(0));
        assert_eq!(core.dense_lane("qwen3e"), Some(1));
        assert!(core.qa_lane("qwen3e").is_some());
        let att = core.attach_sidecar(&split.sidecars[0].bytes).unwrap();
        assert_eq!(att.scopes, vec!["qa", "claims"]);
        assert_eq!(core.dense, index.dense);
        assert_eq!(core.qa_dense, index.qa_dense);
        assert_eq!(core.claims_dense, index.claims_dense);
        // Attaching again is idempotent.
        core.attach_sidecar(&split.sidecars[0].bytes).unwrap();
        assert_eq!(core.qa_dense.len(), 2);
        // Fully attached, it round-trips to a single file again.
        let mut single = Vec::new();
        core.write_ed_to(&mut single).unwrap();
        let back = SearchIndex::from_bytes(&single).unwrap();
        assert!(back.manifest.sidecars.is_empty());
        assert_eq!(back.dense, index.dense);
    }

    #[test]
    fn attach_rejects_foreign_and_corrupt_sidecars() {
        let index = two_lane_index();
        let split = index.to_ed_split("site", &default_split).unwrap();
        let mut core = SearchIndex::from_bytes(&split.core).unwrap();

        // The core file itself is not a sidecar.
        let err = core.attach_sidecar(&split.core).unwrap_err();
        assert!(err.to_string().contains("not a sidecar file"), "{err}");

        // A sidecar from another build (different texts) is refused.
        let other = sample_index();
        let mut b = IndexBuilder::new();
        b.add_chunks(
            other.metadata.clone(),
            vec!["x".into(), "y".into(), "z".into()],
            vec![0, 0, 0],
        )
        .unwrap();
        for lane in &other.dense {
            b.add_dense_lane(SCOPE_CHUNKS, lane.clone()).unwrap();
        }
        b.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(webgpu_spec("qwen3e", 2), 2, 3, &[0.0; 6], Quant::F32).unwrap(),
        )
        .unwrap();
        let foreign = b
            .finish()
            .unwrap()
            .to_ed_split("site", &default_split)
            .unwrap();
        let err = core.attach_sidecar(&foreign.sidecars[0].bytes).unwrap_err();
        assert!(err.to_string().contains("another index build"), "{err}");
        assert!(core.dense_lane("qwen3e").is_none());

        // A flipped payload byte fails the CRC.
        let mut corrupt = split.sidecars[1].bytes.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        let err = core.attach_sidecar(&corrupt).unwrap_err();
        assert!(
            err.to_string().contains("CRC") || err.to_string().contains("decompress"),
            "{err}"
        );

        // A single-file index (0.4.1 layout) has no sidecars and refuses one.
        let mut single = SearchIndex::from_bytes(&ed_bytes(&index)).unwrap();
        assert!(single.manifest.sidecars.is_empty());
        assert_eq!(single.dense.len(), 2);
        let err = single.attach_sidecar(&split.core).unwrap_err();
        assert!(err.to_string().contains("not a sidecar file"), "{err}");
    }

    #[test]
    fn writing_a_single_file_needs_every_chunk_lane() {
        let index = two_lane_index();
        let split = index.to_ed_split("site", &default_split).unwrap();
        let core = SearchIndex::from_bytes(&split.core).unwrap();
        let err = core.write_ed_to(&mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("not attached"), "{err}");
        // Nothing selected means no sidecars and a core equal to the single file.
        let none = index.to_ed_split("site", &|_, _| false).unwrap();
        assert!(none.sidecars.is_empty());
        let m = SearchIndex::manifest_from_bytes(&none.core).unwrap();
        assert!(m.sidecars.is_empty());
        assert_eq!(
            SearchIndex::from_bytes(&none.core).unwrap().dense,
            index.dense
        );
    }

    #[test]
    fn embedded_sparse_vocab_round_trips_and_tokenizes() {
        const JSON: &str = r###"{
          "added_tokens": [
            {"id": 0, "content": "[PAD]", "special": true},
            {"id": 1, "content": "[CLS]", "special": true},
            {"id": 2, "content": "[SEP]", "special": true},
            {"id": 3, "content": "[UNK]", "special": true}
          ],
          "normalizer": {"type": "BertNormalizer", "clean_text": true, "handle_chinese_chars": true, "strip_accents": null, "lowercase": true},
          "pre_tokenizer": {"type": "BertPreTokenizer"},
          "post_processor": {"type": "BertProcessing", "sep": ["[SEP]", 2], "cls": ["[CLS]", 1]},
          "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "max_input_chars_per_word": 100,
            "vocab": {"[PAD]": 0, "[CLS]": 1, "[SEP]": 2, "[UNK]": 3, "rust": 4, "##y": 5, "python": 7, "é": 9}}
        }"###;
        let tokenizer = WordPiece::from_tokenizer_json(JSON.as_bytes(), 512).unwrap();
        let base = sample_index();
        let mut b = IndexBuilder::new();
        b.add_chunks(base.metadata.clone(), base.texts.clone(), vec![0, 0, 0])
            .unwrap();
        for lane in &base.dense {
            b.add_dense_lane(SCOPE_CHUNKS, lane.clone()).unwrap();
        }
        let mut idf = HashMap::new();
        idf.insert(4u32, 1.5f32);
        b.add_sparse(
            &[
                vec![SparseTerm {
                    token_id: 4,
                    weight: 1.0,
                }],
                vec![],
                vec![],
            ],
            &idf,
            base.manifest.sparse.clone().unwrap(),
        )
        .unwrap();
        b.sparse_vocab(tokenizer.clone());
        let index = b.finish().unwrap();
        assert_eq!(
            index.manifest.sparse.as_ref().unwrap().vocab,
            crate::manifest::SparseVocab::Embedded
        );
        let bytes = ed_bytes(&index);
        let back = SearchIndex::from_bytes(&bytes).unwrap();
        let vocab = back.sparse_vocab.as_ref().expect("vocab section");
        for text in ["Rusty Python café", "[SEP] rust", "中文"] {
            assert_eq!(vocab.encode(text), tokenizer.encode(text), "{:?}", text);
        }
        let terms = crate::sparse::sparse_query_terms(
            vocab,
            &|id| back.sparse.as_ref().unwrap().idf_of(id),
            "Rust!",
        );
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].token_id, 4);
        // The section survives the sidecar split (it belongs to the core).
        let split = index.to_ed_split("s", &|_, _| true).unwrap();
        assert!(
            SearchIndex::from_bytes(&split.core)
                .unwrap()
                .sparse_vocab
                .is_some()
        );
        // A manifest that promises an embedded vocab without the section is rejected.
        let info = SearchIndex::inspect(&bytes, None).unwrap();
        assert!(info.sections.iter().any(|s| s.name == "sparse/vocab"));
        // Without the vocab the manifest says fetch (0.4.1 behaviour).
        assert_eq!(
            base.manifest.sparse.as_ref().unwrap().vocab,
            crate::manifest::SparseVocab::Fetch
        );
        assert!(
            SearchIndex::from_bytes(&ed_bytes(&base))
                .unwrap()
                .sparse_vocab
                .is_none()
        );
    }

    #[test]
    fn index_id_tracks_content_and_lanes() {
        let a = sample_index();
        let b = sample_index();
        assert_eq!(a.index_id().unwrap(), b.index_id().unwrap());
        assert_eq!(a.index_id().unwrap().len(), 16);
        let mut c = sample_index();
        c.manifest.dense[0].revision = Some("other".into());
        assert_ne!(a.index_id().unwrap(), c.index_id().unwrap());
        let mut d = sample_index();
        d.texts[0].push('!');
        assert_ne!(a.index_id().unwrap(), d.index_id().unwrap());
        // Written single files carry the id too.
        let m = SearchIndex::manifest_from_bytes(&ed_bytes(&a)).unwrap();
        assert_eq!(m.index_id.as_deref(), Some(a.index_id().unwrap().as_str()));
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
                ..crate::search::Query::default()
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
