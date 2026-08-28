// SPDX-License-Identifier: GPL-3.0-only

//! Index manifest: the model and arm descriptions stored (uncompressed) at the
//! head of an `.ed` file so a runtime can decide what to load before it
//! decompresses anything. Pure data; shared by the indexer, the CLI search
//! path, and the WASM module.

use serde::{Deserialize, Serialize};

/// Index format version written by this crate.
pub const FORMAT_VERSION: u32 = 5;

/// How token states are pooled into one vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pooling {
    Mean,
    Cls,
    /// Last non-padding token (decoder-style embedders such as Qwen3-Embedding).
    Last,
}

/// Model architecture family the native loader must use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Bert,
    XlmRoberta,
    Qwen3,
}

/// Whether a text is a query or a document; selects the instruction prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    Query,
    Document,
}

/// Storage precision of a dense lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quant {
    F32,
    /// Symmetric per-row int8 with one f32 scale per row.
    Int8,
}

/// How a runtime can produce query vectors for a dense lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeSpec {
    /// Candle BERT inside the WASM module (CPU). `files` are fetched from the
    /// HuggingFace repo named by `DenseSpec::model` at `DenseSpec::revision`.
    WasmCandle { files: Vec<String> },
    /// transformers.js ONNX model on WebGPU. `dtype_f16` is used when the
    /// adapter exposes `shader-f16`, otherwise `dtype`.
    WebgpuOnnx {
        repo: String,
        dtype: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dtype_f16: Option<String>,
        /// transformers.js pooling name: `mean`, `cls`, or `last_token`.
        pooling: String,
    },
}

/// One dense embedding lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DenseSpec {
    /// Short lane id used in section names and by the runtime (`minilm`, `qwen3e`).
    pub id: String,
    /// HuggingFace model id.
    pub model: String,
    pub family: Family,
    pub dim: usize,
    pub pooling: Pooling,
    pub normalize: bool,
    #[serde(default)]
    pub query_prefix: String,
    #[serde(default)]
    pub doc_prefix: String,
    pub max_seq_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub quant: Quant,
    pub runtime: RuntimeSpec,
}

impl DenseSpec {
    /// Apply the instruction prefix for `kind`.
    pub fn prefixed(&self, kind: TextKind, text: &str) -> String {
        let prefix = match kind {
            TextKind::Query => &self.query_prefix,
            TextKind::Document => &self.doc_prefix,
        };
        if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{}{}", prefix, text)
        }
    }

    /// HuggingFace revision to resolve files against (`main` when unpinned).
    pub fn revision_or_main(&self) -> &str {
        self.revision.as_deref().unwrap_or("main")
    }
}

/// Learned-sparse arm description. The query side needs only the tokenizer
/// (validated by `vocab_hash`) and the IDF table stored in the index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SparseSpec {
    /// Document encoder model id (informational; not needed at query time).
    pub model: String,
    /// Repo that provides `tokenizer.json` for query tokenization.
    pub tokenizer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Hex SHA-256 of the tokenizer.json bytes used at index time.
    pub vocab_hash: String,
    /// Number of distinct terms in the sparse postings.
    pub terms: usize,
}

/// BM25 parameters used at index time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bm25Params {
    pub k1: f64,
    pub b: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// One weighted term of a sparse vector (query or document side).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SparseTerm {
    pub token_id: u32,
    pub weight: f32,
}

/// Uncompressed header of an `.ed` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// Crate version that wrote the index.
    pub eddie: String,
    pub chunks: usize,
    pub pages: usize,
    #[serde(default)]
    pub dense: Vec<DenseSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse: Option<SparseSpec>,
    #[serde(default)]
    pub bm25: Bm25Params,
    /// Optional payload sections present (`qa`, `claims`).
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_at: Option<String>,
}

impl Manifest {
    pub fn new(chunks: usize, pages: usize) -> Self {
        Self {
            format: FORMAT_VERSION,
            eddie: env!("CARGO_PKG_VERSION").to_string(),
            chunks,
            pages,
            dense: Vec::new(),
            sparse: None,
            bm25: Bm25Params::default(),
            sections: Vec::new(),
            built_at: None,
        }
    }

    pub fn dense_lane(&self, id: &str) -> Option<&DenseSpec> {
        self.dense.iter().find(|d| d.id == id)
    }

    /// First lane the WASM module can run on its own (CPU BERT).
    pub fn first_wasm_lane(&self) -> Option<&DenseSpec> {
        self.dense
            .iter()
            .find(|d| matches!(d.runtime, RuntimeSpec::WasmCandle { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_through_json() {
        let mut m = Manifest::new(10, 3);
        m.dense.push(DenseSpec {
            id: "minilm".into(),
            model: "sentence-transformers/multi-qa-MiniLM-L6-cos-v1".into(),
            family: Family::Bert,
            dim: 384,
            pooling: Pooling::Cls,
            normalize: true,
            query_prefix: String::new(),
            doc_prefix: String::new(),
            max_seq_len: 512,
            revision: Some("abc".into()),
            quant: Quant::Int8,
            runtime: RuntimeSpec::WasmCandle {
                files: vec![
                    "config.json".into(),
                    "tokenizer.json".into(),
                    "model.safetensors".into(),
                ],
            },
        });
        m.dense.push(DenseSpec {
            id: "qwen3e".into(),
            model: "Qwen/Qwen3-Embedding-0.6B".into(),
            family: Family::Qwen3,
            dim: 1024,
            pooling: Pooling::Last,
            normalize: true,
            query_prefix: "Instruct: retrieve\nQuery: ".into(),
            doc_prefix: String::new(),
            max_seq_len: 512,
            revision: None,
            quant: Quant::Int8,
            runtime: RuntimeSpec::WebgpuOnnx {
                repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX".into(),
                dtype: "q4".into(),
                dtype_f16: Some("q4f16".into()),
                pooling: "last_token".into(),
            },
        });
        m.sparse = Some(SparseSpec {
            model: "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill".into(),
            tokenizer: "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill".into(),
            revision: None,
            vocab_hash: "00".into(),
            terms: 5,
        });
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"kind\":\"wasm-candle\""));
        assert!(json.contains("\"kind\":\"webgpu-onnx\""));
        assert!(json.contains("\"pooling\":\"last\""));
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.first_wasm_lane().unwrap().id, "minilm");
        assert_eq!(back.dense_lane("qwen3e").unwrap().dim, 1024);
    }

    #[test]
    fn prefix_applies_only_to_the_requested_kind() {
        let spec = DenseSpec {
            id: "x".into(),
            model: "m".into(),
            family: Family::Qwen3,
            dim: 8,
            pooling: Pooling::Last,
            normalize: true,
            query_prefix: "Q: ".into(),
            doc_prefix: String::new(),
            max_seq_len: 16,
            revision: None,
            quant: Quant::F32,
            runtime: RuntimeSpec::WasmCandle { files: vec![] },
        };
        assert_eq!(spec.prefixed(TextKind::Query, "hi"), "Q: hi");
        assert_eq!(spec.prefixed(TextKind::Document, "hi"), "hi");
        assert_eq!(spec.revision_or_main(), "main");
    }
}
