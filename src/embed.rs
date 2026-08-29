// SPDX-License-Identifier: GPL-3.0-only

//! Dense embedding inference.
//!
//! One [`DenseEncoder`] per model family, all driven by the same tokenizer
//! discipline: no fixed padding, explicit truncation at the model's
//! `max_seq_len`, batches padded to the longest item with an attention mask.
//!
//! - [`BertEncoder`]: sentence-transformers BERT checkpoints (MiniLM, BGE v1.5,
//!   GTE, E5, Arctic). Runs natively and inside the WASM module.
//! - [`XlmRobertaEncoder`]: XLM-RoBERTa checkpoints such as `BAAI/bge-m3`
//!   (native only; loads `pytorch_model.bin`).
//! - [`Qwen3Encoder`]: decoder-style embedders (`Qwen/Qwen3-Embedding-0.6B`,
//!   `microsoft/harrier-oss-v1-0.6b`) with last-token pooling (native only).

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use candle_core::{D, DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use tokenizers::{Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

pub use crate::manifest::{DenseSpec, Family, Pooling, Quant, RuntimeSpec, TextKind};
use crate::models;

/// Default number of texts per forward pass.
pub const DEFAULT_BATCH_SIZE: usize = 32;
/// Sequence cap applied when a repo carries no `sentence_bert_config.json`.
pub const DEFAULT_MAX_SEQ_LEN: usize = 512;

/// A loaded dense embedding model.
pub trait DenseEncoder: Send + Sync {
    /// Lane description (model id, pooling, prefixes, dim, runtime).
    fn spec(&self) -> &DenseSpec;

    /// Embed `texts`. Applies the prefix for `kind`, truncates at
    /// `spec().max_seq_len`, runs padded batches, pools per `spec().pooling`
    /// and L2-normalises when `spec().normalize`.
    fn embed(&self, texts: &[&str], kind: TextKind) -> Result<Vec<Vec<f32>>>;

    /// Number of inputs truncated since construction (or the last reset).
    fn truncated_count(&self) -> usize;

    /// Reset the truncation counter.
    fn reset_truncated_count(&self);

    /// Number of wordpieces (special tokens included, no truncation) the
    /// model's tokenizer produces for `text`; the chunker's token budget.
    fn count_tokens(&self, text: &str) -> usize;

    /// Output dimensionality.
    fn dim(&self) -> usize {
        self.spec().dim
    }
}

/// Configure a tokenizer the way every encoder expects: no padding block
/// (batches are padded by the caller) and explicit right truncation.
pub fn prepare_tokenizer(tokenizer: &mut Tokenizer, max_seq_len: usize) -> Result<()> {
    tokenizer.with_padding(None);
    tokenizer
        .with_truncation(Some(TruncationParams {
            direction: TruncationDirection::Right,
            max_length: max_seq_len.max(1),
            strategy: TruncationStrategy::LongestFirst,
            stride: 0,
        }))
        .map_err(|e| anyhow!("configuring tokenizer truncation: {e}"))?;
    Ok(())
}

/// Untruncated token count for chunk sizing (0 when the tokenizer fails).
fn count_tokens_with(tokenizer: &Tokenizer, text: &str) -> usize {
    tokenizer
        .encode(text, true)
        .map(|enc| {
            enc.get_ids().len()
                + enc
                    .get_overflowing()
                    .iter()
                    .map(|o| o.get_ids().len())
                    .sum::<usize>()
        })
        .unwrap_or(0)
}

/// Tokenize a batch. Returns token ids per text and how many were truncated.
fn encode_all(tokenizer: &Tokenizer, texts: &[String]) -> Result<(Vec<Vec<u32>>, usize)> {
    let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let encodings = tokenizer
        .encode_batch(inputs, true)
        .map_err(|e| anyhow!("tokenizer error: {e}"))?;
    let mut truncated = 0usize;
    let ids = encodings
        .into_iter()
        .map(|enc| {
            if !enc.get_overflowing().is_empty() {
                truncated += 1;
            }
            enc.get_ids().to_vec()
        })
        .collect();
    Ok((ids, truncated))
}

/// `[batch, seq]` id and mask tensors for a right-padded batch.
struct PaddedBatch {
    input_ids: Tensor,
    attention_mask: Tensor,
    lengths: Vec<usize>,
}

fn pad_batch(ids: &[&[u32]], pad_id: u32, device: &Device) -> Result<PaddedBatch> {
    let batch = ids.len();
    let max_len = ids.iter().map(|x| x.len()).max().unwrap_or(0).max(1);
    let mut flat_ids = Vec::with_capacity(batch * max_len);
    let mut flat_mask = Vec::with_capacity(batch * max_len);
    let mut lengths = Vec::with_capacity(batch);
    for row in ids {
        lengths.push(row.len().max(1));
        flat_ids.extend_from_slice(row);
        flat_mask.extend(std::iter::repeat_n(1u32, row.len()));
        let pad = max_len - row.len();
        flat_ids.extend(std::iter::repeat_n(pad_id, pad));
        flat_mask.extend(std::iter::repeat_n(0u32, pad));
    }
    Ok(PaddedBatch {
        input_ids: Tensor::from_vec(flat_ids, (batch, max_len), device)?,
        attention_mask: Tensor::from_vec(flat_mask, (batch, max_len), device)?,
        lengths,
    })
}

/// Pool `[batch, seq, hidden]` token states into `[batch, hidden]`.
fn pool(hidden: &Tensor, mask: &Tensor, lengths: &[usize], pooling: Pooling) -> Result<Tensor> {
    match pooling {
        Pooling::Cls => Ok(hidden.i((.., 0, ..))?.contiguous()?),
        Pooling::Mean => {
            let mask = mask.unsqueeze(2)?.to_dtype(hidden.dtype())?;
            let summed = hidden.broadcast_mul(&mask)?.sum(1)?;
            let counts = mask.sum(1)?.clamp(1e-9, f64::MAX)?;
            Ok(summed.broadcast_div(&counts)?)
        }
        Pooling::Last => {
            let (b, _l, h) = hidden.dims3()?;
            let last: Vec<u32> = lengths.iter().map(|l| (l - 1) as u32).collect();
            let idx = Tensor::from_vec(last, (b, 1, 1), hidden.device())?
                .broadcast_as((b, 1, h))?
                .contiguous()?;
            Ok(hidden.gather(&idx, 1)?.squeeze(1)?)
        }
    }
}

fn l2_normalize(x: &Tensor) -> Result<Tensor> {
    let norm = x
        .sqr()?
        .sum_keepdim(D::Minus1)?
        .sqrt()?
        .clamp(1e-12, f64::MAX)?;
    Ok(x.broadcast_div(&norm)?)
}

fn finish(pooled: &Tensor, normalize: bool) -> Result<Vec<Vec<f32>>> {
    let pooled = pooled.to_dtype(DType::F32)?;
    let pooled = if normalize {
        l2_normalize(&pooled)?
    } else {
        pooled
    };
    Ok(pooled.to_vec2::<f32>()?)
}

/// Run `forward` over length-sorted batches and restore the input order.
/// `same_length_only` cuts a batch whenever the token length changes
/// (needed by models that cannot take a padding mask).
fn run_batched<F>(
    ids: &[Vec<u32>],
    batch_size: usize,
    same_length_only: bool,
    forward: F,
) -> Result<Vec<Vec<f32>>>
where
    F: Fn(&[&[u32]]) -> Result<Vec<Vec<f32>>>,
{
    let batch_size = batch_size.max(1);
    let n = ids.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| ids[i].len());
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); n];
    let mut start = 0;
    while start < n {
        let mut end = (start + batch_size).min(n);
        if same_length_only {
            let len = ids[order[start]].len();
            end = order[start..end]
                .iter()
                .position(|&i| ids[i].len() != len)
                .map(|p| start + p)
                .unwrap_or(end);
        }
        let batch: Vec<&[u32]> = order[start..end]
            .iter()
            .map(|&i| ids[i].as_slice())
            .collect();
        let vecs = forward(&batch)?;
        if vecs.len() != batch.len() {
            bail!(
                "encoder returned {} vectors for a batch of {}",
                vecs.len(),
                batch.len()
            );
        }
        for (&i, v) in order[start..end].iter().zip(vecs) {
            out[i] = v;
        }
        start = end;
    }
    Ok(out)
}

/// Fill in `dim` / `max_seq_len` from the model config when the spec did not
/// carry them, and clamp `max_seq_len` to what the model can position-embed.
fn finalize_spec(spec: &mut DenseSpec, hidden_size: usize, max_position_embeddings: usize) {
    spec.dim = hidden_size;
    let cap = max_position_embeddings.max(1);
    spec.max_seq_len = if spec.max_seq_len == 0 {
        DEFAULT_MAX_SEQ_LEN.min(cap)
    } else {
        spec.max_seq_len.min(cap)
    };
}

fn prefixed_all(spec: &DenseSpec, texts: &[&str], kind: TextKind) -> Vec<String> {
    texts.iter().map(|t| spec.prefixed(kind, t)).collect()
}

// ---------------------------------------------------------------------------
// BERT
// ---------------------------------------------------------------------------

/// BERT sentence encoder (native and WASM).
pub struct BertEncoder {
    model: BertModel,
    tokenizer: Tokenizer,
    spec: DenseSpec,
    device: Device,
    pad_id: u32,
    batch_size: usize,
    truncated: AtomicUsize,
}

impl BertEncoder {
    /// Build from an already-parsed config, tokenizer and weight source.
    pub fn from_var_builder(
        mut spec: DenseSpec,
        config: &BertConfig,
        mut tokenizer: Tokenizer,
        vb: VarBuilder,
        device: Device,
        batch_size: usize,
    ) -> Result<Self> {
        if let Some(model_type) = &config.model_type
            && model_type != "bert"
        {
            bail!(
                "{}: model_type '{}' is not a BERT checkpoint (supported families: bert, xlm-roberta, qwen3)",
                spec.model,
                model_type
            );
        }
        if spec.family != Family::Bert {
            bail!(
                "lane '{}' is family {:?} but was given to the BERT loader",
                spec.id,
                spec.family
            );
        }
        finalize_spec(
            &mut spec,
            config.hidden_size,
            config.max_position_embeddings,
        );
        prepare_tokenizer(&mut tokenizer, spec.max_seq_len)?;
        let model = BertModel::load(vb, config).context("building BertModel")?;
        Ok(Self {
            model,
            tokenizer,
            spec,
            device,
            pad_id: config.pad_token_id as u32,
            batch_size: batch_size.max(1),
            truncated: AtomicUsize::new(0),
        })
    }

    fn forward_batch(&self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        let padded = pad_batch(batch, self.pad_id, &self.device)?;
        let token_type_ids = padded.input_ids.zeros_like()?;
        let hidden = self.model.forward(
            &padded.input_ids,
            &token_type_ids,
            Some(&padded.attention_mask),
        )?;
        let pooled = pool(
            &hidden,
            &padded.attention_mask,
            &padded.lengths,
            self.spec.pooling,
        )?;
        finish(&pooled, self.spec.normalize)
    }
}

impl DenseEncoder for BertEncoder {
    fn spec(&self) -> &DenseSpec {
        &self.spec
    }

    fn embed(&self, texts: &[&str], kind: TextKind) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inputs = prefixed_all(&self.spec, texts, kind);
        let (ids, truncated) = encode_all(&self.tokenizer, &inputs)?;
        self.truncated.fetch_add(truncated, Ordering::Relaxed);
        run_batched(&ids, self.batch_size, false, |b| self.forward_batch(b))
    }

    fn truncated_count(&self) -> usize {
        self.truncated.load(Ordering::Relaxed)
    }

    fn reset_truncated_count(&self) {
        self.truncated.store(0, Ordering::Relaxed);
    }

    fn count_tokens(&self, text: &str) -> usize {
        count_tokens_with(&self.tokenizer, text)
    }
}

/// A `DenseSpec` skeleton for a BERT lane whose config has not been read yet
/// (`dim` and `max_seq_len` are filled in by the constructor).
pub fn bert_spec_skeleton(model: &str) -> DenseSpec {
    let defaults = models::lookup(model);
    DenseSpec {
        id: models::lane_id_for(model),
        model: model.to_string(),
        family: Family::Bert,
        dim: 0,
        pooling: defaults.map(|d| d.pooling).unwrap_or(Pooling::Mean),
        normalize: true,
        query_prefix: defaults.map(|d| d.query_prefix).unwrap_or("").to_string(),
        doc_prefix: defaults.map(|d| d.doc_prefix).unwrap_or("").to_string(),
        max_seq_len: 0,
        revision: None,
        quant: Quant::Int8,
        runtime: RuntimeSpec::WasmCandle {
            files: models::WASM_CANDLE_FILES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
    }
}

/// Build a BERT encoder from raw `config.json`, `tokenizer.json` and
/// `model.safetensors` bytes (the WASM path; works natively too).
pub fn bert_from_bytes(
    spec: DenseSpec,
    config: &[u8],
    tokenizer: &[u8],
    weights: Vec<u8>,
) -> Result<BertEncoder> {
    let config: BertConfig = serde_json::from_slice(config).context("parsing config.json")?;
    let tokenizer =
        Tokenizer::from_bytes(tokenizer).map_err(|e| anyhow!("parsing tokenizer.json: {e}"))?;
    let device = Device::Cpu;
    let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)
        .context("loading model weights")?;
    BertEncoder::from_var_builder(spec, &config, tokenizer, vb, device, DEFAULT_BATCH_SIZE)
}

// ---------------------------------------------------------------------------
// Compatibility shim (old single-model API used by wasm.rs and the search CLI)
// ---------------------------------------------------------------------------

/// Thin wrapper over a [`DenseEncoder`] with the pre-0.4 method names.
/// Kept so `wasm.rs` and the search commands compile until they move to
/// the lane-aware API; new code should use [`load_dense`] / [`bert_from_bytes`].
pub struct Embedder {
    inner: Box<dyn DenseEncoder>,
}

impl Embedder {
    /// Load a model from the HuggingFace Hub on the CPU.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(model_id: &str) -> Result<Self> {
        let inner = load_dense(model_id, &Device::Cpu, &DenseOverrides::default())?;
        Ok(Self { inner })
    }

    /// Build a BERT encoder from raw file bytes (mean pooling unless the
    /// model id is known to the registry, which this path cannot see).
    pub fn from_bytes(
        config_bytes: &[u8],
        tokenizer_bytes: &[u8],
        weights_bytes: Vec<u8>,
    ) -> Result<Self> {
        let spec = bert_spec_skeleton(models::DEFAULT_DENSE_MODEL);
        let inner = bert_from_bytes(spec, config_bytes, tokenizer_bytes, weights_bytes)?;
        Ok(Self {
            inner: Box::new(inner),
        })
    }

    /// Wrap any encoder.
    pub fn from_encoder(inner: Box<dyn DenseEncoder>) -> Self {
        Self { inner }
    }

    /// Embed texts as queries.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.inner.embed(texts, TextKind::Query)
    }

    pub fn dim(&self) -> usize {
        self.inner.dim()
    }

    pub fn encoder(&self) -> &dyn DenseEncoder {
        self.inner.as_ref()
    }

    pub fn count_tokens(&self, text: &str) -> usize {
        self.inner.count_tokens(text)
    }
}

// ---------------------------------------------------------------------------
// Native: device selection, hub access, XLM-RoBERTa and Qwen3 encoders
// ---------------------------------------------------------------------------

/// Where to run inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevicePref {
    /// CUDA when compiled in and a GPU answers, else CPU.
    #[default]
    Auto,
    Cpu,
    Cuda(usize),
}

impl std::str::FromStr for DevicePref {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "auto" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            "cuda" | "gpu" => Ok(Self::Cuda(0)),
            other => {
                if let Some(idx) = other.strip_prefix("cuda:") {
                    let idx: usize = idx
                        .parse()
                        .with_context(|| format!("invalid CUDA device index '{idx}'"))?;
                    Ok(Self::Cuda(idx))
                } else {
                    bail!("unknown device '{other}' (expected auto, cpu, cuda or cuda:N)")
                }
            }
        }
    }
}

/// Pick the compute device for `pref`. CUDA is only available when the crate
/// was built with `--features cuda`.
pub fn select_device(pref: DevicePref) -> Result<Device> {
    match pref {
        DevicePref::Cpu => Ok(Device::Cpu),
        DevicePref::Cuda(idx) => cuda_device(idx),
        DevicePref::Auto => {
            if cfg!(feature = "cuda") {
                match cuda_device(0) {
                    Ok(d) => Ok(d),
                    Err(err) => {
                        eprintln!("  CUDA unavailable ({err:#}); using CPU");
                        Ok(Device::Cpu)
                    }
                }
            } else {
                Ok(Device::Cpu)
            }
        }
    }
}

#[cfg(feature = "cuda")]
fn cuda_device(idx: usize) -> Result<Device> {
    Device::new_cuda(idx).with_context(|| format!("initialising CUDA device {idx}"))
}

#[cfg(not(feature = "cuda"))]
fn cuda_device(_idx: usize) -> Result<Device> {
    bail!(
        "this eddie binary was built without the `cuda` feature; rebuild with `cargo build --release --features cuda`"
    )
}

/// Human-readable device name for log lines.
pub fn device_name(device: &Device) -> String {
    match device {
        Device::Cpu => "cpu".to_string(),
        Device::Cuda(_) => "cuda".to_string(),
        Device::Metal(_) => "metal".to_string(),
    }
}

/// Caller overrides applied on top of the repo files and the registry.
#[derive(Debug, Clone, Default)]
pub struct DenseOverrides {
    pub lane_id: Option<String>,
    pub family: Option<Family>,
    pub pooling: Option<Pooling>,
    pub max_seq_len: Option<usize>,
    pub query_prefix: Option<String>,
    pub doc_prefix: Option<String>,
    pub normalize: Option<bool>,
    pub runtime: Option<RuntimeSpec>,
    /// Pin a HuggingFace revision (branch, tag or commit).
    pub revision: Option<String>,
    pub batch_size: Option<usize>,
}

/// HuggingFace Hub access shared by the dense and sparse loaders.
#[cfg(not(target_arch = "wasm32"))]
pub mod hub {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};
    use hf_hub::HFError;
    use hf_hub::repository::RepoTypeModel;
    use hf_hub::{HFClientSync, HFRepositorySync};

    /// One model repository at an optional pinned revision.
    pub struct ModelRepo {
        repo: HFRepositorySync<RepoTypeModel>,
        model_id: String,
        revision: Option<String>,
    }

    impl ModelRepo {
        /// Open `owner/name`. Honors `HF_HOME`, `HF_HUB_CACHE`, `HF_TOKEN`
        /// and `HF_ENDPOINT` like the Python client.
        pub fn open(model_id: &str, revision: Option<&str>) -> Result<Self> {
            let (owner, name) = model_id
                .split_once('/')
                .with_context(|| format!("model id '{model_id}' must look like 'owner/name'"))?;
            if owner.is_empty() || name.is_empty() {
                bail!("model id '{model_id}' must look like 'owner/name'");
            }
            let client = HFClientSync::new().context("creating HuggingFace Hub client")?;
            Ok(Self {
                repo: client.model(owner, name),
                model_id: model_id.to_string(),
                revision: revision.map(|s| s.to_string()),
            })
        }

        pub fn model_id(&self) -> &str {
            &self.model_id
        }

        /// Download (or reuse from the cache) one file.
        pub fn get(&self, file: &str) -> Result<PathBuf> {
            self.repo
                .download_file()
                .filename(file)
                .maybe_revision(self.revision.clone())
                .send()
                .with_context(|| format!("downloading {}/{}", self.model_id, file))
        }

        /// Like [`get`](Self::get) but `Ok(None)` when the repo has no such file.
        pub fn get_optional(&self, file: &str) -> Result<Option<PathBuf>> {
            match self
                .repo
                .download_file()
                .filename(file)
                .maybe_revision(self.revision.clone())
                .send()
            {
                Ok(p) => Ok(Some(p)),
                Err(HFError::EntryNotFound { .. }) => Ok(None),
                Err(HFError::Http { context, .. }) if context.status.as_u16() == 404 => Ok(None),
                Err(e) => Err(e).with_context(|| format!("downloading {}/{}", self.model_id, file)),
            }
        }

        /// Read a small file to a string.
        pub fn read_string(&self, file: &str) -> Result<String> {
            let path = self.get(file)?;
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
        }

        /// Read an optional small file to a string.
        pub fn read_optional_string(&self, file: &str) -> Result<Option<String>> {
            match self.get_optional(file)? {
                Some(path) => Ok(Some(
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?,
                )),
                None => Ok(None),
            }
        }
    }

    /// Commit sha a cached file resolved to (`.../snapshots/<sha>/<file>`).
    pub fn revision_from_path(path: &Path) -> Option<String> {
        let mut comps = path.components().peekable();
        while let Some(c) = comps.next() {
            if c.as_os_str() == "snapshots" {
                return comps
                    .next()
                    .map(|s| s.as_os_str().to_string_lossy().to_string());
            }
        }
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::PathBuf;

    use super::*;
    use crate::embed::hub::{ModelRepo, revision_from_path};
    use candle_transformers::models::qwen3::{Config as Qwen3Config, Model as Qwen3Model};
    use candle_transformers::models::xlm_roberta::{Config as XlmRobertaConfig, XLMRobertaModel};

    /// XLM-RoBERTa encoder (e.g. `BAAI/bge-m3`, CLS pooling).
    pub struct XlmRobertaEncoder {
        model: XLMRobertaModel,
        tokenizer: Tokenizer,
        spec: DenseSpec,
        device: Device,
        pad_id: u32,
        batch_size: usize,
        truncated: AtomicUsize,
    }

    impl XlmRobertaEncoder {
        pub fn from_var_builder(
            mut spec: DenseSpec,
            config: &XlmRobertaConfig,
            mut tokenizer: Tokenizer,
            vb: VarBuilder,
            device: Device,
            batch_size: usize,
        ) -> Result<Self> {
            // Position ids start after the padding index, so the usable length
            // is two shorter than the embedding table.
            let cap = config.max_position_embeddings.saturating_sub(2).max(1);
            finalize_spec(&mut spec, config.hidden_size, cap);
            prepare_tokenizer(&mut tokenizer, spec.max_seq_len)?;
            let model = match XLMRobertaModel::new(config, vb.clone()) {
                Ok(m) => m,
                Err(_) => XLMRobertaModel::new(config, vb.pp("roberta"))
                    .context("building XLMRobertaModel")?,
            };
            Ok(Self {
                model,
                tokenizer,
                spec,
                device,
                pad_id: config.pad_token_id,
                batch_size: batch_size.max(1),
                truncated: AtomicUsize::new(0),
            })
        }

        fn forward_batch(&self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
            let padded = pad_batch(batch, self.pad_id, &self.device)?;
            let token_type_ids = padded.input_ids.zeros_like()?;
            let hidden = self.model.forward(
                &padded.input_ids,
                &padded.attention_mask,
                &token_type_ids,
                None,
                None,
                None,
            )?;
            let pooled = pool(
                &hidden,
                &padded.attention_mask,
                &padded.lengths,
                self.spec.pooling,
            )?;
            finish(&pooled, self.spec.normalize)
        }
    }

    impl DenseEncoder for XlmRobertaEncoder {
        fn spec(&self) -> &DenseSpec {
            &self.spec
        }

        fn embed(&self, texts: &[&str], kind: TextKind) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let inputs = prefixed_all(&self.spec, texts, kind);
            let (ids, truncated) = encode_all(&self.tokenizer, &inputs)?;
            self.truncated.fetch_add(truncated, Ordering::Relaxed);
            run_batched(&ids, self.batch_size, false, |b| self.forward_batch(b))
        }

        fn truncated_count(&self) -> usize {
            self.truncated.load(Ordering::Relaxed)
        }

        fn reset_truncated_count(&self) {
            self.truncated.store(0, Ordering::Relaxed);
        }

        fn count_tokens(&self, text: &str) -> usize {
            count_tokens_with(&self.tokenizer, text)
        }
    }

    /// Qwen3 decoder used as an embedder (last-token pooling).
    ///
    /// candle's Qwen3 has no padding mask, so a batch only ever contains
    /// texts of identical token length; the KV cache is discarded per batch
    /// by cloning the (Arc-backed, cheap) pristine model.
    pub struct Qwen3Encoder {
        model: Qwen3Model,
        tokenizer: Tokenizer,
        spec: DenseSpec,
        device: Device,
        batch_size: usize,
        truncated: AtomicUsize,
    }

    impl Qwen3Encoder {
        pub fn from_var_builder(
            mut spec: DenseSpec,
            config: &Qwen3Config,
            mut tokenizer: Tokenizer,
            vb: VarBuilder,
            device: Device,
            batch_size: usize,
        ) -> Result<Self> {
            finalize_spec(
                &mut spec,
                config.hidden_size,
                config.max_position_embeddings,
            );
            prepare_tokenizer(&mut tokenizer, spec.max_seq_len)?;
            let model = Qwen3Model::new(config, vb).context("building Qwen3 model")?;
            Ok(Self {
                model,
                tokenizer,
                spec,
                device,
                batch_size: batch_size.max(1),
                truncated: AtomicUsize::new(0),
            })
        }

        fn forward_batch(&self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
            let b = batch.len();
            let l = batch[0].len().max(1);
            let mut flat = Vec::with_capacity(b * l);
            for row in batch {
                if row.len() != l {
                    bail!("Qwen3 batches must have equal token lengths");
                }
                flat.extend_from_slice(row);
            }
            let input = Tensor::from_vec(flat, (b, l), &self.device)?;
            // Fresh KV cache per batch: clone the pristine model.
            let mut model = self.model.clone();
            let hidden = model.forward(&input, 0)?;
            let pooled = match self.spec.pooling {
                Pooling::Last => hidden.i((.., l - 1, ..))?.contiguous()?,
                other => {
                    let mask = Tensor::ones((b, l), DType::U32, &self.device)?;
                    let lengths = vec![l; b];
                    pool(&hidden, &mask, &lengths, other)?
                }
            };
            finish(&pooled, self.spec.normalize)
        }
    }

    impl DenseEncoder for Qwen3Encoder {
        fn spec(&self) -> &DenseSpec {
            &self.spec
        }

        fn embed(&self, texts: &[&str], kind: TextKind) -> Result<Vec<Vec<f32>>> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let inputs = prefixed_all(&self.spec, texts, kind);
            let (ids, truncated) = encode_all(&self.tokenizer, &inputs)?;
            self.truncated.fetch_add(truncated, Ordering::Relaxed);
            // One text per forward pass: candle's qwen3 model returns
            // non-finite hidden states for batches larger than one (seen on
            // CUDA in bf16, f16 and f32 with equal-length batches), and a
            // batch of one already runs at ~140 texts/s on a GPU.
            let _ = self.batch_size;
            run_batched(&ids, 1, true, |b| self.forward_batch(b))
        }

        fn truncated_count(&self) -> usize {
            self.truncated.load(Ordering::Relaxed)
        }

        fn reset_truncated_count(&self) {
            self.truncated.store(0, Ordering::Relaxed);
        }

        fn count_tokens(&self, text: &str) -> usize {
            count_tokens_with(&self.tokenizer, text)
        }
    }

    /// Model weights as downloaded from the hub.
    enum Weights {
        Safetensors(Vec<PathBuf>),
        Pth(PathBuf),
    }

    fn fetch_weights(repo: &ModelRepo) -> Result<Weights> {
        if let Some(p) = repo.get_optional("model.safetensors")? {
            return Ok(Weights::Safetensors(vec![p]));
        }
        if let Some(index) = repo.read_optional_string("model.safetensors.index.json")? {
            let index: serde_json::Value =
                serde_json::from_str(&index).context("parsing model.safetensors.index.json")?;
            let mut files: Vec<String> = index["weight_map"]
                .as_object()
                .context("model.safetensors.index.json has no weight_map")?
                .values()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            files.sort();
            files.dedup();
            let mut paths = Vec::with_capacity(files.len());
            for f in files {
                paths.push(repo.get(&f)?);
            }
            return Ok(Weights::Safetensors(paths));
        }
        if let Some(p) = repo.get_optional("pytorch_model.bin")? {
            return Ok(Weights::Pth(p));
        }
        bail!(
            "{}: no model.safetensors, sharded safetensors index or pytorch_model.bin found",
            repo.model_id()
        )
    }

    fn var_builder<'a>(weights: &Weights, dtype: DType, device: &Device) -> Result<VarBuilder<'a>> {
        match weights {
            Weights::Safetensors(paths) => {
                // SAFETY: the files are only read; they are not modified while mapped.
                unsafe { VarBuilder::from_mmaped_safetensors(paths, dtype, device) }
                    .context("loading safetensors weights")
            }
            Weights::Pth(path) => {
                VarBuilder::from_pth(path, dtype, device).context("loading pytorch_model.bin")
            }
        }
    }

    /// Pooling from a sentence-transformers `1_Pooling/config.json`.
    fn pooling_from_config(json: &str) -> Option<Pooling> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        if flag("pooling_mode_cls_token") {
            Some(Pooling::Cls)
        } else if flag("pooling_mode_mean_tokens") {
            Some(Pooling::Mean)
        } else if flag("pooling_mode_lasttoken") {
            Some(Pooling::Last)
        } else {
            None
        }
    }

    fn max_seq_len_from_sbert(json: &str) -> Option<usize> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        v.get("max_seq_length")?.as_u64().map(|n| n as usize)
    }

    /// Load a dense encoder from the HuggingFace Hub.
    ///
    /// Reads `config.json`, `tokenizer.json`, `1_Pooling/config.json` and
    /// `sentence_bert_config.json` (the last two when present); the model
    /// registry supplies prefixes, lane id and browser runtime for known ids;
    /// `overrides` win over both.
    pub fn load_dense(
        model: &str,
        device: &Device,
        overrides: &DenseOverrides,
    ) -> Result<Box<dyn DenseEncoder>> {
        let repo = ModelRepo::open(model, overrides.revision.as_deref())?;
        let defaults = models::lookup(model);

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

        let family = overrides
            .family
            .or(defaults.map(|d| d.family))
            .or_else(|| models::family_from_model_type(&model_type))
            .with_context(|| {
                format!(
                    "{model}: unsupported architecture '{}' (supported: bert, xlm-roberta, qwen3)",
                    if model_type.is_empty() {
                        "unknown"
                    } else {
                        &model_type
                    }
                )
            })?;

        let tokenizer_path = repo.get("tokenizer.json")?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("parsing {}: {e}", tokenizer_path.display()))?;

        let repo_pooling = repo
            .read_optional_string("1_Pooling/config.json")?
            .and_then(|s| pooling_from_config(&s));
        let repo_max_seq = repo
            .read_optional_string("sentence_bert_config.json")?
            .and_then(|s| max_seq_len_from_sbert(&s));

        let pooling = overrides
            .pooling
            .or(repo_pooling)
            .or(defaults.map(|d| d.pooling))
            .unwrap_or_else(|| models::family_default_pooling(family));

        let runtime = match (&overrides.runtime, defaults, family) {
            (Some(r), _, _) => r.clone(),
            (None, Some(d), _) => d.runtime_spec(),
            (None, None, Family::Bert) => models::runtime_spec(models::RuntimeKind::WasmCandle),
            (None, None, other) => bail!(
                "{model}: {other:?} models need a browser runtime; pass --dense-runtime webgpu-onnx:<repo>:<dtype>[:<dtype_f16>] (or use a registry model)"
            ),
        };

        let revision = overrides
            .revision
            .clone()
            .or_else(|| revision_from_path(&config_path));

        let spec = DenseSpec {
            id: overrides
                .lane_id
                .clone()
                .unwrap_or_else(|| models::lane_id_for(model)),
            model: model.to_string(),
            family,
            dim: 0,
            pooling,
            normalize: overrides.normalize.unwrap_or(true),
            query_prefix: overrides
                .query_prefix
                .clone()
                .unwrap_or_else(|| defaults.map(|d| d.query_prefix).unwrap_or("").to_string()),
            doc_prefix: overrides
                .doc_prefix
                .clone()
                .unwrap_or_else(|| defaults.map(|d| d.doc_prefix).unwrap_or("").to_string()),
            max_seq_len: overrides.max_seq_len.or(repo_max_seq).unwrap_or(0),
            revision,
            quant: Quant::Int8,
            runtime,
        };
        let batch_size = overrides.batch_size.unwrap_or(DEFAULT_BATCH_SIZE);
        let weights = fetch_weights(&repo)?;

        match family {
            Family::Bert => {
                let config: BertConfig =
                    serde_json::from_str(&config_json).context("parsing BERT config.json")?;
                let vb = var_builder(&weights, DType::F32, device)?;
                Ok(Box::new(BertEncoder::from_var_builder(
                    spec,
                    &config,
                    tokenizer,
                    vb,
                    device.clone(),
                    batch_size,
                )?))
            }
            Family::XlmRoberta => {
                let config: XlmRobertaConfig = serde_json::from_str(&config_json)
                    .context("parsing XLM-RoBERTa config.json")?;
                let vb = var_builder(&weights, DType::F32, device)?;
                Ok(Box::new(XlmRobertaEncoder::from_var_builder(
                    spec,
                    &config,
                    tokenizer,
                    vb,
                    device.clone(),
                    batch_size,
                )?))
            }
            Family::Qwen3 => {
                let config: Qwen3Config =
                    serde_json::from_str(&config_json).context("parsing Qwen3 config.json")?;
                let dtype = qwen3_dtype(device);
                let vb = var_builder(&weights, dtype, device)?;
                // Embedding checkpoints ship `embed_tokens.*` / `layers.*`
                // without the `model.` prefix candle's Qwen3 expects.
                let vb = if vb.contains_tensor("model.embed_tokens.weight") {
                    vb
                } else if vb.contains_tensor("embed_tokens.weight") {
                    vb.rename_f(|name: &str| {
                        name.strip_prefix("model.").unwrap_or(name).to_string()
                    })
                } else {
                    bail!(
                        "{model}: weights contain neither model.embed_tokens.weight nor embed_tokens.weight"
                    )
                };
                Ok(Box::new(Qwen3Encoder::from_var_builder(
                    spec,
                    &config,
                    tokenizer,
                    vb,
                    device.clone(),
                    batch_size,
                )?))
            }
        }
    }

    /// f32 on CPU (candle's CPU flash attention runs in f32), bf16 on CUDA.
    pub fn qwen3_dtype(device: &Device) -> DType {
        if let Ok(v) = std::env::var("EDDIE_QWEN3_DTYPE") {
            match v.to_ascii_lowercase().as_str() {
                "bf16" => return DType::BF16,
                "f16" => return DType::F16,
                "f32" => return DType::F32,
                _ => {}
            }
        }
        if device.is_cuda() {
            DType::BF16
        } else {
            DType::F32
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{Qwen3Encoder, XlmRobertaEncoder, load_dense, qwen3_dtype};

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb)
    }

    #[test]
    fn run_batched_restores_order_and_groups_lengths() {
        let ids: Vec<Vec<u32>> = vec![vec![1, 2, 3], vec![1], vec![4, 5, 6], vec![7, 8]];
        let out = run_batched(&ids, 8, true, |batch| {
            let len = batch[0].len();
            assert!(batch.iter().all(|b| b.len() == len));
            Ok(batch.iter().map(|b| vec![b[0] as f32]).collect())
        })
        .unwrap();
        assert_eq!(out, vec![vec![1.0], vec![1.0], vec![4.0], vec![7.0]]);
    }

    #[test]
    fn pooling_modes_respect_the_mask() {
        let device = Device::Cpu;
        // batch 2, seq 3, hidden 2
        let hidden = Tensor::new(
            &[
                [[1.0f32, 2.0], [3.0, 4.0], [100.0, 100.0]],
                [[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]],
            ],
            &device,
        )
        .unwrap();
        let mask = Tensor::new(&[[1u32, 1, 0], [1, 1, 1]], &device).unwrap();
        let lengths = [2usize, 3];
        let mean = pool(&hidden, &mask, &lengths, Pooling::Mean)
            .unwrap()
            .to_vec2::<f32>()
            .unwrap();
        assert_eq!(mean, vec![vec![2.0, 3.0], vec![7.0, 8.0]]);
        let cls = pool(&hidden, &mask, &lengths, Pooling::Cls)
            .unwrap()
            .to_vec2::<f32>()
            .unwrap();
        assert_eq!(cls, vec![vec![1.0, 2.0], vec![5.0, 6.0]]);
        let last = pool(&hidden, &mask, &lengths, Pooling::Last)
            .unwrap()
            .to_vec2::<f32>()
            .unwrap();
        assert_eq!(last, vec![vec![3.0, 4.0], vec![9.0, 10.0]]);
    }

    #[test]
    fn device_pref_parses() {
        assert_eq!("auto".parse::<DevicePref>().unwrap(), DevicePref::Auto);
        assert_eq!("CPU".parse::<DevicePref>().unwrap(), DevicePref::Cpu);
        assert_eq!("cuda:1".parse::<DevicePref>().unwrap(), DevicePref::Cuda(1));
        assert!("tpu".parse::<DevicePref>().is_err());
    }

    #[test]
    fn finalize_spec_caps_sequence_length() {
        let mut spec = bert_spec_skeleton("x/y");
        finalize_spec(&mut spec, 384, 512);
        assert_eq!((spec.dim, spec.max_seq_len), (384, 512));
        spec.max_seq_len = 8192;
        finalize_spec(&mut spec, 384, 512);
        assert_eq!(spec.max_seq_len, 512);
        spec.max_seq_len = 128;
        finalize_spec(&mut spec, 384, 512);
        assert_eq!(spec.max_seq_len, 128);
    }

    #[test]
    #[ignore] // requires network access and the HuggingFace cache
    fn minilm_loads_with_mean_pooling_and_no_fixed_padding() {
        let enc = load_dense(
            models::DEFAULT_DENSE_MODEL,
            &Device::Cpu,
            &DenseOverrides::default(),
        )
        .unwrap();
        assert_eq!(enc.dim(), 384);
        assert_eq!(enc.spec().pooling, Pooling::Mean);
        assert_eq!(enc.spec().max_seq_len, 512);
        assert!(enc.spec().revision.is_some(), "revision should be pinned");
        let vecs = enc
            .embed(
                &[
                    "Rust programming language",
                    "writing Rust code",
                    "chocolate cake recipe",
                ],
                TextKind::Document,
            )
            .unwrap();
        assert!(cosine(&vecs[0], &vecs[1]) > cosine(&vecs[0], &vecs[2]));
        assert!((vecs[0].iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(enc.truncated_count(), 0);
    }

    /// Sentences shared with `scripts/verify_embeddings.py`.
    pub(crate) const VERIFY_TEXTS: [&str; 5] = [
        "The quick brown fox jumps over the lazy dog.",
        "How do I configure the search widget on a Hugo site?",
        "Eddie builds a semantic index at build time and searches it in the browser.",
        "Photosynthesis converts light energy into chemical energy in plants.",
        "The 2024 release added CUDA support for indexing.",
    ];

    /// Cross-implementation check against sentence-transformers.
    ///
    /// Procedure:
    /// 1. `uv venv ~/tmp/st-venv && uv pip install --python ~/tmp/st-venv/bin/python sentence-transformers torch transformers`
    /// 2. `~/tmp/st-venv/bin/python scripts/verify_embeddings.py --out ~/tmp/eddie-ref`
    ///    (writes `<lane>.json` with `docs` / `queries` vectors per model)
    /// 3. `EDDIE_REF_DIR=~/tmp/eddie-ref cargo test --release -- --ignored compare_with_sentence_transformers`
    ///    (set `EDDIE_DEVICE=cuda` to run the candle side on the GPU with `--features cuda`)
    ///
    /// Thresholds: cosine(eddie, reference) >= 0.999 for BERT / XLM-RoBERTa
    /// lanes and >= 0.99 for Qwen3, for every sentence, documents and queries.
    #[test]
    #[ignore] // requires network access, the HuggingFace cache and EDDIE_REF_DIR
    fn compare_with_sentence_transformers() {
        let ref_dir = std::env::var("EDDIE_REF_DIR").expect("set EDDIE_REF_DIR");
        let device = match std::env::var("EDDIE_DEVICE").as_deref() {
            Ok(pref) => select_device(pref.parse().unwrap()).unwrap(),
            Err(_) => Device::Cpu,
        };
        let lanes: Vec<String> = match std::env::var("EDDIE_LANES") {
            Ok(l) => l.split(',').map(|s| s.trim().to_string()).collect(),
            Err(_) => vec![
                "minilm".into(),
                "bge-small".into(),
                "bge-m3".into(),
                "qwen3e".into(),
            ],
        };
        let mut worst: Vec<(String, f32, f32)> = Vec::new();
        for lane in &lanes {
            let path = format!("{ref_dir}/{lane}.json");
            let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("reading {path}: {e}; run scripts/verify_embeddings.py")
            });
            let reference: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let model = reference["model"].as_str().unwrap();
            let enc = load_dense(model, &device, &DenseOverrides::default()).unwrap();
            let threshold = match enc.spec().family {
                Family::Qwen3 => 0.99,
                _ => 0.999,
            };
            let mut min_doc = 1.0f32;
            let mut min_query = 1.0f32;
            for (key, kind, min) in [
                ("docs", TextKind::Document, &mut min_doc),
                ("queries", TextKind::Query, &mut min_query),
            ] {
                let expected: Vec<Vec<f32>> =
                    serde_json::from_value(reference[key].clone()).unwrap();
                let got = enc.embed(&VERIFY_TEXTS, kind).unwrap();
                assert_eq!(got.len(), expected.len());
                for (i, (g, e)) in got.iter().zip(&expected).enumerate() {
                    assert_eq!(g.len(), e.len(), "{lane}: dim mismatch");
                    let c = cosine(g, e);
                    *min = min.min(c);
                    assert!(
                        c >= threshold,
                        "{lane} {key}[{i}]: cosine {c:.5} < {threshold}"
                    );
                }
            }
            eprintln!(
                "{lane} ({model}) on {}: min cosine docs={min_doc:.5} queries={min_query:.5} (threshold {threshold})",
                device_name(&device)
            );
            worst.push((lane.clone(), min_doc, min_query));
        }
        assert!(!worst.is_empty());
    }
}
