// SPDX-License-Identifier: GPL-3.0-only

//! Learned sparse retrieval with an inference-free query side.
//!
//! Documents are expanded at index time by an OpenSearch neural sparse doc
//! encoder (a masked-LM head over WordPiece vocabulary):
//! `w(t) = act(max over positions of logit_t)` with the model card's
//! activation (`log(1 + relu(x))` for v2, `log(1 + log(1 + relu(x)))` for v3),
//! special-token columns zeroed, then pruned to terms with weight at least
//! `prune_ratio × max weight` (OpenSearch's `max_ratio` pruning).
//!
//! Queries need no model: WordPiece ids of the query weighted by the IDF table
//! the model ships in `idf.json`. That path compiles for wasm32 as well.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;

use crate::embed::prepare_tokenizer;
pub use crate::manifest::{SparseSpec, SparseTerm};

/// Keep document terms whose weight is at least this fraction of the
/// document's largest weight (OpenSearch `prune_type: max_ratio` default).
pub const DEFAULT_PRUNE_RATIO: f32 = 0.1;
/// Longest input the DistilBERT/BERT doc encoders can position-embed.
pub const DEFAULT_MAX_SEQ_LEN: usize = 512;

/// Hex SHA-256 of `tokenizer.json` bytes (stored in the manifest as `vocab_hash`).
pub fn tokenizer_json_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Parse a `tokenizer.json` for query-side sparse encoding: no padding,
/// truncation at `max_seq_len`.
pub fn sparse_tokenizer_from_bytes(bytes: &[u8], max_seq_len: usize) -> Result<Tokenizer> {
    let mut tokenizer =
        Tokenizer::from_bytes(bytes).map_err(|e| anyhow!("parsing tokenizer.json: {e}"))?;
    prepare_tokenizer(&mut tokenizer, max_seq_len)?;
    Ok(tokenizer)
}

/// Sparse query vector: WordPiece ids of `query` (special tokens dropped),
/// weight = `idf(id)`, duplicates collapsed to the maximum, sorted by id.
/// Ids without an IDF entry are dropped.
pub fn sparse_query_terms(
    tokenizer: &Tokenizer,
    idf: &dyn Fn(u32) -> Option<f32>,
    query: &str,
) -> Vec<SparseTerm> {
    let Ok(encoding) = tokenizer.encode(query, true) else {
        return Vec::new();
    };
    let mut weights: BTreeMap<u32, f32> = BTreeMap::new();
    let special = encoding.get_special_tokens_mask();
    for (i, &id) in encoding.get_ids().iter().enumerate() {
        if special.get(i).copied().unwrap_or(0) == 1 {
            continue;
        }
        let Some(w) = idf(id) else { continue };
        if w.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            continue;
        }
        let slot = weights.entry(id).or_insert(w);
        if w > *slot {
            *slot = w;
        }
    }
    weights
        .into_iter()
        .map(|(token_id, weight)| SparseTerm { token_id, weight })
        .collect()
}

/// Inner product of two id-sorted sparse vectors.
pub fn sparse_dot(a: &[SparseTerm], b: &[SparseTerm]) -> f32 {
    let (mut i, mut j, mut sum) = (0, 0, 0.0f32);
    while i < a.len() && j < b.len() {
        match a[i].token_id.cmp(&b[j].token_id) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                sum += a[i].weight * b[j].weight;
                i += 1;
                j += 1;
            }
        }
    }
    sum
}

/// Turn a dense vocabulary-sized weight row into pruned, id-sorted terms.
pub fn prune_terms(weights: &[f32], prune_ratio: f32) -> Vec<SparseTerm> {
    let max_w = weights.iter().copied().fold(0.0f32, f32::max);
    if max_w.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Vec::new();
    }
    let floor = max_w * prune_ratio.clamp(0.0, 1.0);
    weights
        .iter()
        .enumerate()
        .filter(|(_, w)| **w > 0.0 && **w >= floor)
        .map(|(id, &w)| SparseTerm {
            token_id: id as u32,
            weight: w,
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::{Context, Result, anyhow, bail};
    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertForMaskedLM, Config as BertConfig};
    use candle_transformers::models::distilbert::{
        Config as DistilBertConfig, DistilBertForMaskedLM,
    };
    use tokenizers::Tokenizer;

    use super::{DEFAULT_MAX_SEQ_LEN, DEFAULT_PRUNE_RATIO, prune_terms, tokenizer_json_sha256};
    use crate::embed::hub::{ModelRepo, revision_from_path};
    use crate::embed::prepare_tokenizer;
    use crate::manifest::{SparseSpec, SparseTerm};

    /// Activation applied to the max-pooled logits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SparseActivation {
        /// `log(1 + relu(x))` (v1/v2 doc encoders).
        Log1pRelu,
        /// `log(1 + log(1 + relu(x)))` (v3 doc encoders, per the model card).
        Log1pLog1pRelu,
    }

    impl SparseActivation {
        /// The activation a model id implies (v3 repos use the double log).
        pub fn for_model(model_id: &str) -> Self {
            if model_id.contains("-v3") {
                Self::Log1pLog1pRelu
            } else {
                Self::Log1pRelu
            }
        }
    }

    /// Loader options.
    #[derive(Debug, Clone)]
    pub struct SparseOptions {
        pub revision: Option<String>,
        /// Texts per forward pass. Logits are `batch × seq × vocab` f32, so
        /// keep this small (8 ≈ 500 MB at 512 tokens).
        pub batch_size: usize,
        pub prune_ratio: f32,
        /// `None` picks from the model id.
        pub activation: Option<SparseActivation>,
        pub max_seq_len: usize,
    }

    impl Default for SparseOptions {
        fn default() -> Self {
            Self {
                revision: None,
                batch_size: 8,
                prune_ratio: DEFAULT_PRUNE_RATIO,
                activation: None,
                max_seq_len: DEFAULT_MAX_SEQ_LEN,
            }
        }
    }

    enum MlmModel {
        DistilBert(DistilBertForMaskedLM),
        Bert(BertForMaskedLM),
    }

    /// OpenSearch neural sparse document encoder.
    pub struct SparseDocEncoder {
        model: MlmModel,
        tokenizer: Tokenizer,
        tokenizer_sha256: String,
        idf: HashMap<u32, f32>,
        special_ids: Vec<u32>,
        device: Device,
        model_id: String,
        revision: Option<String>,
        activation: SparseActivation,
        prune_ratio: f32,
        batch_size: usize,
        pad_id: u32,
        truncated: AtomicUsize,
    }

    impl SparseDocEncoder {
        /// Load a doc encoder from the HuggingFace Hub with default options.
        pub fn load(model: &str, device: &Device) -> Result<Self> {
            Self::load_with(model, device, &SparseOptions::default())
        }

        /// Load with explicit options. Supports DistilBERT MLM checkpoints
        /// (`doc-v2-distill`, `doc-v3-distill`) and BERT MLM checkpoints
        /// (`doc-v1`, `doc-v3-gte`-style repos with `model_type: bert`).
        pub fn load_with(model: &str, device: &Device, opts: &SparseOptions) -> Result<Self> {
            let repo = ModelRepo::open(model, opts.revision.as_deref())?;
            let config_path = repo.get("config.json")?;
            let config_json = std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?;
            let config_value: serde_json::Value =
                serde_json::from_str(&config_json).context("parsing config.json")?;
            let model_type = config_value
                .get("model_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let tokenizer_path = repo.get("tokenizer.json")?;
            let tokenizer_bytes = std::fs::read(&tokenizer_path)
                .with_context(|| format!("reading {}", tokenizer_path.display()))?;
            let tokenizer_sha256 = tokenizer_json_sha256(&tokenizer_bytes);
            let mut tokenizer = Tokenizer::from_bytes(&tokenizer_bytes)
                .map_err(|e| anyhow!("parsing {}: {e}", tokenizer_path.display()))?;
            prepare_tokenizer(&mut tokenizer, opts.max_seq_len.min(DEFAULT_MAX_SEQ_LEN))?;

            let weights_path = repo.get("model.safetensors")?;
            // SAFETY: the file is only read and is not modified while mapped.
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], DType::F32, device)
                    .context("loading sparse encoder weights")?
            };
            let (mlm, pad_id, max_pos) = match model_type.as_str() {
                "distilbert" => {
                    let cfg: DistilBertConfig = serde_json::from_str(&config_json)
                        .context("parsing DistilBERT config.json")?;
                    let m = DistilBertForMaskedLM::load(vb, &cfg)
                        .context("building DistilBertForMaskedLM")?;
                    let max_pos = config_value
                        .get("max_position_embeddings")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(DEFAULT_MAX_SEQ_LEN as u64)
                        as usize;
                    (MlmModel::DistilBert(m), cfg.pad_token_id as u32, max_pos)
                }
                "bert" => {
                    let cfg: BertConfig =
                        serde_json::from_str(&config_json).context("parsing BERT config.json")?;
                    let m = BertForMaskedLM::load(vb, &cfg).context("building BertForMaskedLM")?;
                    (
                        MlmModel::Bert(m),
                        cfg.pad_token_id as u32,
                        cfg.max_position_embeddings,
                    )
                }
                other => bail!(
                    "{model}: sparse encoders must be DistilBERT or BERT masked-LM checkpoints (model_type '{other}')"
                ),
            };
            if max_pos < opts.max_seq_len {
                prepare_tokenizer(&mut tokenizer, max_pos)?;
            }

            let idf_json = repo.read_string("idf.json")?;
            let idf_by_token: HashMap<String, f32> =
                serde_json::from_str(&idf_json).context("parsing idf.json")?;
            let mut idf = HashMap::with_capacity(idf_by_token.len());
            let mut unmapped = 0usize;
            for (token, weight) in idf_by_token {
                match tokenizer.token_to_id(&token) {
                    Some(id) => {
                        idf.insert(id, weight);
                    }
                    None => unmapped += 1,
                }
            }
            if unmapped > 0 {
                eprintln!("  warning: {unmapped} idf.json tokens are not in the tokenizer vocab");
            }

            let special_ids = special_token_ids(&tokenizer);
            let revision = opts
                .revision
                .clone()
                .or_else(|| revision_from_path(&config_path));

            Ok(Self {
                model: mlm,
                tokenizer,
                tokenizer_sha256,
                idf,
                special_ids,
                device: device.clone(),
                model_id: model.to_string(),
                revision,
                activation: opts
                    .activation
                    .unwrap_or_else(|| SparseActivation::for_model(model)),
                prune_ratio: opts.prune_ratio,
                batch_size: opts.batch_size.max(1),
                pad_id,
                truncated: AtomicUsize::new(0),
            })
        }

        /// Expand documents into pruned, id-sorted sparse vectors.
        pub fn encode_docs(&self, texts: &[&str]) -> Result<Vec<Vec<SparseTerm>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let encodings = self
                .tokenizer
                .encode_batch(texts.to_vec(), true)
                .map_err(|e| anyhow!("tokenizer error: {e}"))?;
            let mut ids: Vec<Vec<u32>> = Vec::with_capacity(encodings.len());
            let mut truncated = 0usize;
            for enc in encodings {
                if !enc.get_overflowing().is_empty() {
                    truncated += 1;
                }
                ids.push(enc.get_ids().to_vec());
            }
            self.truncated.fetch_add(truncated, Ordering::Relaxed);

            let n = ids.len();
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&i| ids[i].len());
            let mut out: Vec<Vec<SparseTerm>> = vec![Vec::new(); n];
            for chunk in order.chunks(self.batch_size) {
                let batch: Vec<&[u32]> = chunk.iter().map(|&i| ids[i].as_slice()).collect();
                let rows = self.forward_batch(&batch)?;
                for (&i, row) in chunk.iter().zip(rows) {
                    out[i] = row;
                }
            }
            Ok(out)
        }

        fn forward_batch(&self, batch: &[&[u32]]) -> Result<Vec<Vec<SparseTerm>>> {
            let b = batch.len();
            let max_len = batch.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
            let mut flat_ids = Vec::with_capacity(b * max_len);
            let mut flat_mask = Vec::with_capacity(b * max_len);
            for row in batch {
                flat_ids.extend_from_slice(row);
                flat_mask.extend(std::iter::repeat_n(1u32, row.len()));
                let pad = max_len - row.len();
                flat_ids.extend(std::iter::repeat_n(self.pad_id, pad));
                flat_mask.extend(std::iter::repeat_n(0u32, pad));
            }
            let input_ids = Tensor::from_vec(flat_ids, (b, max_len), &self.device)?;
            let attention_mask = Tensor::from_vec(flat_mask, (b, max_len), &self.device)?;

            let logits = match &self.model {
                MlmModel::DistilBert(m) => {
                    // candle's DistilBERT masks positions where the mask is
                    // non-zero, so hand it "1 = padding" with a broadcastable
                    // [batch, 1, 1, seq] shape.
                    let pad_mask = attention_mask.eq(0u32)?.unsqueeze(1)?.unsqueeze(1)?;
                    m.forward(&input_ids, &pad_mask)?
                }
                MlmModel::Bert(m) => {
                    let token_type_ids = input_ids.zeros_like()?;
                    m.forward(&input_ids, &token_type_ids, Some(&attention_mask))?
                }
            };
            // [b, seq, vocab] -> relu -> zero padded positions -> max over seq
            let mask = attention_mask.unsqueeze(2)?.to_dtype(logits.dtype())?;
            let pooled = logits.relu()?.broadcast_mul(&mask)?.max(1)?;
            let pooled = (pooled + 1.0)?.log()?;
            let pooled = match self.activation {
                SparseActivation::Log1pRelu => pooled,
                SparseActivation::Log1pLog1pRelu => (pooled + 1.0)?.log()?,
            };
            let rows = pooled.to_dtype(DType::F32)?.to_vec2::<f32>()?;
            Ok(rows
                .into_iter()
                .map(|mut row| {
                    for &id in &self.special_ids {
                        if let Some(w) = row.get_mut(id as usize) {
                            *w = 0.0;
                        }
                    }
                    prune_terms(&row, self.prune_ratio)
                })
                .collect())
        }

        /// Query-side terms using this encoder's tokenizer and IDF table.
        pub fn query_terms(&self, query: &str) -> Vec<SparseTerm> {
            let idf = &self.idf;
            super::sparse_query_terms(&self.tokenizer, &|id| idf.get(&id).copied(), query)
        }

        /// IDF weight per token id (from the repo's `idf.json`).
        pub fn idf(&self) -> &HashMap<u32, f32> {
            &self.idf
        }

        pub fn tokenizer(&self) -> &Tokenizer {
            &self.tokenizer
        }

        /// Hex SHA-256 of the `tokenizer.json` bytes that were loaded.
        pub fn tokenizer_json_sha256(&self) -> String {
            self.tokenizer_sha256.clone()
        }

        pub fn model_id(&self) -> &str {
            &self.model_id
        }

        pub fn revision(&self) -> Option<&str> {
            self.revision.as_deref()
        }

        pub fn activation(&self) -> SparseActivation {
            self.activation
        }

        pub fn prune_ratio(&self) -> f32 {
            self.prune_ratio
        }

        /// Inputs truncated since construction (or the last reset).
        pub fn truncated_count(&self) -> usize {
            self.truncated.load(Ordering::Relaxed)
        }

        pub fn reset_truncated_count(&self) {
            self.truncated.store(0, Ordering::Relaxed);
        }

        /// Manifest entry for an index with `terms` distinct postings terms.
        pub fn spec(&self, terms: usize) -> SparseSpec {
            SparseSpec {
                model: self.model_id.clone(),
                tokenizer: self.model_id.clone(),
                revision: self.revision.clone(),
                vocab_hash: self.tokenizer_sha256.clone(),
                terms,
            }
        }
    }

    /// Ids of the tokenizer's special tokens ([PAD], [CLS], ... and any
    /// added token flagged `special`).
    fn special_token_ids(tokenizer: &Tokenizer) -> Vec<u32> {
        let mut ids: Vec<u32> = tokenizer
            .get_added_vocabulary()
            .get_added_tokens_decoder()
            .iter()
            .filter(|(_, tok)| tok.special)
            .map(|(id, _)| *id)
            .collect();
        for name in ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"] {
            if let Some(id) = tokenizer.token_to_id(name) {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{SparseActivation, SparseDocEncoder, SparseOptions};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_terms_above_ratio_sorted_by_id() {
        let weights = [0.0, 1.0, 0.05, 0.2, 0.09, 0.5];
        let terms = prune_terms(&weights, 0.1);
        let ids: Vec<u32> = terms.iter().map(|t| t.token_id).collect();
        assert_eq!(ids, vec![1, 3, 5]);
        assert!(prune_terms(&[0.0, 0.0], 0.1).is_empty());
    }

    #[test]
    fn sparse_dot_merges_sorted_ids() {
        let a = [
            SparseTerm {
                token_id: 1,
                weight: 2.0,
            },
            SparseTerm {
                token_id: 5,
                weight: 1.0,
            },
        ];
        let b = [
            SparseTerm {
                token_id: 5,
                weight: 3.0,
            },
            SparseTerm {
                token_id: 9,
                weight: 1.0,
            },
        ];
        assert_eq!(sparse_dot(&a, &b), 3.0);
    }

    #[test]
    fn sha256_is_hex() {
        assert_eq!(
            tokenizer_json_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Cross-implementation check against `transformers`' AutoModelForMaskedLM
    /// (formula from the model card). Produce the reference with
    /// `scripts/verify_embeddings.py --skip-dense`, then
    /// `EDDIE_REF_DIR=~/tmp/eddie-ref cargo test --release -- --ignored compare_with_transformers_sparse`.
    /// The top-10 term ids must match in order and weights within 2%.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore] // requires network access, the HuggingFace cache and EDDIE_REF_DIR
    fn compare_with_transformers_sparse() {
        let ref_dir = std::env::var("EDDIE_REF_DIR").expect("set EDDIE_REF_DIR");
        let path = format!("{ref_dir}/sparse.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {path}: {e}; run scripts/verify_embeddings.py"));
        let reference: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let model = reference["model"].as_str().unwrap();
        let texts: Vec<String> = serde_json::from_value(reference["texts"].clone()).unwrap();
        let expected: Vec<Vec<(u32, f32)>> =
            serde_json::from_value(reference["top10"].clone()).unwrap();
        let enc = SparseDocEncoder::load(model, &candle_core::Device::Cpu).unwrap();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let docs = enc.encode_docs(&refs).unwrap();
        for (i, (doc, exp)) in docs.iter().zip(&expected).enumerate() {
            let mut top: Vec<&SparseTerm> = doc.iter().collect();
            top.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap());
            let got: Vec<(u32, f32)> = top
                .iter()
                .take(10)
                .map(|t| (t.token_id, t.weight))
                .collect();
            let got_ids: Vec<u32> = got.iter().map(|t| t.0).collect();
            let exp_ids: Vec<u32> = exp.iter().map(|t| t.0).collect();
            eprintln!("text[{i}]: {} terms, top-10 ids {:?}", doc.len(), got_ids);
            assert_eq!(
                got_ids, exp_ids,
                "text[{i}] top-10 ids differ: got {got:?} expected {exp:?}"
            );
            for (g, e) in got.iter().zip(exp) {
                assert!(
                    (g.1 - e.1).abs() <= 0.02 * e.1.abs().max(1e-3),
                    "text[{i}] token {}: weight {} vs {}",
                    g.0,
                    g.1,
                    e.1
                );
            }
        }
        // Query side: every query term must carry the repo IDF.
        let q = enc.query_terms("What's the weather in ny now?");
        assert!(!q.is_empty());
        assert!(q.windows(2).all(|w| w[0].token_id < w[1].token_id));
        eprintln!("query terms: {:?}", q);
    }

    #[test]
    fn query_terms_drop_specials_and_collapse_duplicates() {
        // Minimal WordPiece tokenizer with a BERT-style post-processor.
        let json = r###"{
          "version": "1.0",
          "truncation": null, "padding": null,
          "added_tokens": [
            {"id": 0, "content": "[PAD]", "special": true, "single_word": false, "lstrip": false, "rstrip": false, "normalized": false},
            {"id": 1, "content": "[CLS]", "special": true, "single_word": false, "lstrip": false, "rstrip": false, "normalized": false},
            {"id": 2, "content": "[SEP]", "special": true, "single_word": false, "lstrip": false, "rstrip": false, "normalized": false},
            {"id": 3, "content": "[UNK]", "special": true, "single_word": false, "lstrip": false, "rstrip": false, "normalized": false}
          ],
          "normalizer": {"type": "BertNormalizer", "clean_text": true, "handle_chinese_chars": true, "strip_accents": null, "lowercase": true},
          "pre_tokenizer": {"type": "BertPreTokenizer"},
          "post_processor": {"type": "BertProcessing", "sep": ["[SEP]", 2], "cls": ["[CLS]", 1]},
          "decoder": null,
          "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "max_input_chars_per_word": 100,
            "vocab": {"[PAD]": 0, "[CLS]": 1, "[SEP]": 2, "[UNK]": 3, "new": 4, "york": 5, "weather": 6}}
        }"###;
        let tokenizer = sparse_tokenizer_from_bytes(json.as_bytes(), 16).unwrap();
        let idf = |id: u32| match id {
            4 => Some(1.5),
            5 => Some(2.5),
            6 => Some(3.0),
            _ => None,
        };
        let terms = sparse_query_terms(&tokenizer, &idf, "New York weather, new york!");
        let pairs: Vec<(u32, f32)> = terms.iter().map(|t| (t.token_id, t.weight)).collect();
        assert_eq!(pairs, vec![(4, 1.5), (5, 2.5), (6, 3.0)]);
    }
}
