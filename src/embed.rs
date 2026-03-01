// SPDX-License-Identifier: GPL-3.0-only

//! Embedding inference: run a sentence-transformer model to produce vectors.

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
#[cfg(not(target_arch = "wasm32"))]
use hf_hub::api::sync::Api;
use tokenizers::Tokenizer;

/// Sentence embedding model backed by Candle BERT inference.
pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    config: Config,
    device: Device,
}

impl Embedder {
    /// Load a sentence-transformer model from HuggingFace Hub.
    ///
    /// Downloads `config.json`, `tokenizer.json`, and `model.safetensors`
    /// for the given model ID (e.g. `sentence-transformers/all-MiniLM-L6-v2`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(model_id: &str) -> Result<Self> {
        let device = Device::Cpu;
        let api = Api::new().context("creating HuggingFace Hub API client")?;
        let repo = api.model(model_id.to_string());

        let config_path = repo
            .get("config.json")
            .context("downloading config.json")?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .context("downloading tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("downloading model.safetensors")?;

        let config: Config = serde_json::from_str(
            &std::fs::read_to_string(&config_path).context("reading config.json")?,
        )
        .context("parsing config.json")?;

        let tokenizer =
            Tokenizer::from_file(&tokenizer_path).map_err(|e| anyhow::anyhow!("{}", e))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
                .context("loading model weights")?
        };

        let model = BertModel::load(vb, &config).context("building BertModel")?;

        Ok(Self {
            model,
            tokenizer,
            config,
            device,
        })
    }

    /// Embed a batch of texts, returning one L2-normalized vector per text.
    /// Each vector has `self.dim()` dimensions.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());

        for text in texts {
            let encoding = self
                .tokenizer
                .encode(*text, true)
                .map_err(|e| anyhow::anyhow!("tokenizer error: {}", e))?;

            let ids = encoding.get_ids();
            let attention = encoding.get_attention_mask();

            let input_ids =
                Tensor::new(ids, &self.device)?.unsqueeze(0)?;
            let token_type_ids = input_ids.zeros_like()?;
            let attention_mask =
                Tensor::new(attention, &self.device)?.unsqueeze(0)?;

            // Forward pass: [1, seq_len, hidden_size]
            let output = self
                .model
                .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

            // Mean pooling with attention mask
            let embedding = mean_pool(&output, &attention_mask)?;

            // L2 normalize
            let embedding = l2_normalize(&embedding)?;

            let vec: Vec<f32> = embedding.squeeze(0)?.to_vec1()?;
            results.push(vec);
        }

        Ok(results)
    }

    /// The dimensionality of output embeddings (hidden_size from config).
    pub fn dim(&self) -> usize {
        self.config.hidden_size
    }
}

/// Mean pooling: sum token embeddings weighted by attention mask, divide by mask sum.
/// Input shapes: output [batch, seq_len, hidden], mask [batch, seq_len].
fn mean_pool(output: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
    // Expand mask to [batch, seq_len, 1] for broadcasting
    let mask_expanded = attention_mask
        .unsqueeze(2)?
        .to_dtype(output.dtype())?;

    // Multiply and sum over seq_len dimension
    let masked = output.broadcast_mul(&mask_expanded)?;
    let summed = masked.sum(1)?; // [batch, hidden]

    // Sum of mask per position
    let mask_sum = mask_expanded.sum(1)?; // [batch, 1]
    let mask_sum = mask_sum.clamp(1e-9, f64::MAX)?; // avoid division by zero

    let pooled = summed.broadcast_div(&mask_sum)?;
    Ok(pooled)
}

/// L2-normalize each vector in the batch.
/// Input shape: [batch, hidden].
fn l2_normalize(tensor: &Tensor) -> Result<Tensor> {
    let norm = tensor
        .sqr()?
        .sum_keepdim(1)?
        .sqrt()?
        .clamp(1e-12, f64::MAX)?;
    let normalized = tensor.broadcast_div(&norm)?;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";

    #[test]
    #[ignore] // requires network access to download model
    fn test_embedding_dimensions() {
        let embedder = Embedder::new(MODEL_ID).unwrap();
        assert_eq!(embedder.dim(), 384);

        let vecs = embedder.embed_batch(&["Hello world"]).unwrap();
        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0].len(), 384);
    }

    #[test]
    #[ignore] // requires network access to download model
    fn test_embedding_similarity() {
        let embedder = Embedder::new(MODEL_ID).unwrap();
        let vecs = embedder
            .embed_batch(&[
                "Rust programming language",
                "writing Rust code",
                "chocolate cake recipe",
            ])
            .unwrap();

        let sim_related = dot(&vecs[0], &vecs[1]);
        let sim_unrelated = dot(&vecs[0], &vecs[2]);

        assert!(
            sim_related > sim_unrelated,
            "related similarity ({}) should be greater than unrelated ({})",
            sim_related,
            sim_unrelated
        );
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }
}
