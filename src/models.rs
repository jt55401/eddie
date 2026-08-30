// SPDX-License-Identifier: GPL-3.0-only

//! Registry of known dense embedding models: the lane id, architecture family,
//! pooling, instruction prefixes and browser runtime that Eddie uses for each
//! HuggingFace model id. Values here are defaults; the native loader still
//! reads `1_Pooling/config.json` and `sentence_bert_config.json` from the
//! repo and lets CLI overrides win.

use crate::manifest::{Family, Pooling, RuntimeSpec};

/// Query prefix used by the BGE v1.5 and Snowflake Arctic families.
pub const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";
/// Query instruction used by Qwen3-Embedding and Harrier (documents get none).
pub const QWEN3_QUERY_PREFIX: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

/// Files the WASM candle runtime fetches for a BERT lane.
pub const WASM_CANDLE_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// Default sparse document encoder.
pub const DEFAULT_SPARSE_MODEL: &str =
    "opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill";
/// Default dense lane.
pub const DEFAULT_DENSE_MODEL: &str = "sentence-transformers/multi-qa-MiniLM-L6-cos-v1";

/// Browser runtime a registry entry recommends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    /// Candle BERT inside the WASM module.
    WasmCandle,
    /// transformers.js ONNX port on WebGPU.
    WebgpuOnnx {
        repo: &'static str,
        dtype: &'static str,
        dtype_f16: Option<&'static str>,
        /// transformers.js pooling name.
        pooling: &'static str,
    },
}

/// Static defaults for one known model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDefaults {
    /// Short lane id (`minilm`, `bge-small`, `qwen3e`, ...).
    pub lane_id: &'static str,
    pub model: &'static str,
    pub family: Family,
    /// Pooling when the repo has no `1_Pooling/config.json`.
    pub pooling: Pooling,
    pub query_prefix: &'static str,
    pub doc_prefix: &'static str,
    pub runtime: RuntimeKind,
}

impl ModelDefaults {
    pub fn runtime_spec(&self) -> RuntimeSpec {
        runtime_spec(self.runtime)
    }
}

pub fn runtime_spec(kind: RuntimeKind) -> RuntimeSpec {
    match kind {
        RuntimeKind::WasmCandle => RuntimeSpec::WasmCandle {
            files: WASM_CANDLE_FILES.iter().map(|s| s.to_string()).collect(),
            base_url: None,
        },
        RuntimeKind::WebgpuOnnx {
            repo,
            dtype,
            dtype_f16,
            pooling,
        } => RuntimeSpec::WebgpuOnnx {
            repo: repo.to_string(),
            dtype: dtype.to_string(),
            dtype_f16: dtype_f16.map(|s| s.to_string()),
            pooling: pooling.to_string(),
            base_url: None,
        },
    }
}

const fn bert(
    lane_id: &'static str,
    model: &'static str,
    pooling: Pooling,
    query_prefix: &'static str,
    doc_prefix: &'static str,
) -> ModelDefaults {
    ModelDefaults {
        lane_id,
        model,
        family: Family::Bert,
        pooling,
        query_prefix,
        doc_prefix,
        runtime: RuntimeKind::WasmCandle,
    }
}

/// Known models. Pooling values were checked against each repo's
/// `1_Pooling/config.json` on 2026-08-28.
pub const REGISTRY: &[ModelDefaults] = &[
    bert(
        "minilm",
        "sentence-transformers/multi-qa-MiniLM-L6-cos-v1",
        Pooling::Mean,
        "",
        "",
    ),
    bert(
        "minilm-l6",
        "sentence-transformers/all-MiniLM-L6-v2",
        Pooling::Mean,
        "",
        "",
    ),
    bert(
        "minilm-l12",
        "sentence-transformers/all-MiniLM-L12-v2",
        Pooling::Mean,
        "",
        "",
    ),
    bert(
        "bge-small",
        "BAAI/bge-small-en-v1.5",
        Pooling::Cls,
        BGE_QUERY_PREFIX,
        "",
    ),
    bert("gte-small", "thenlper/gte-small", Pooling::Mean, "", ""),
    bert(
        "arctic-s",
        "Snowflake/snowflake-arctic-embed-s",
        Pooling::Cls,
        BGE_QUERY_PREFIX,
        "",
    ),
    bert(
        "e5-small",
        "intfloat/e5-small-v2",
        Pooling::Mean,
        "query: ",
        "passage: ",
    ),
    bert(
        "e5-base",
        "intfloat/e5-base-v2",
        Pooling::Mean,
        "query: ",
        "passage: ",
    ),
    ModelDefaults {
        lane_id: "bge-m3",
        model: "BAAI/bge-m3",
        family: Family::XlmRoberta,
        pooling: Pooling::Cls,
        query_prefix: "",
        doc_prefix: "",
        runtime: RuntimeKind::WebgpuOnnx {
            repo: "Xenova/bge-m3",
            dtype: "q8",
            dtype_f16: None,
            pooling: "cls",
        },
    },
    ModelDefaults {
        lane_id: "qwen3e",
        model: "Qwen/Qwen3-Embedding-0.6B",
        family: Family::Qwen3,
        pooling: Pooling::Last,
        query_prefix: QWEN3_QUERY_PREFIX,
        doc_prefix: "",
        runtime: RuntimeKind::WebgpuOnnx {
            repo: "onnx-community/Qwen3-Embedding-0.6B-ONNX",
            dtype: "q4",
            dtype_f16: Some("q4f16"),
            pooling: "last_token",
        },
    },
    ModelDefaults {
        lane_id: "harrier",
        model: "microsoft/harrier-oss-v1-0.6b",
        family: Family::Qwen3,
        pooling: Pooling::Last,
        query_prefix: QWEN3_QUERY_PREFIX,
        doc_prefix: "",
        runtime: RuntimeKind::WebgpuOnnx {
            repo: "onnx-community/harrier-oss-v1-0.6b-ONNX",
            dtype: "q4",
            dtype_f16: Some("q4f16"),
            pooling: "last_token",
        },
    },
];

/// Look a model id up in the registry (exact, case-sensitive match).
pub fn lookup(model_id: &str) -> Option<&'static ModelDefaults> {
    REGISTRY.iter().find(|m| m.model == model_id)
}

/// Resolve a lane id or model id typed by a user to a registry entry.
pub fn lookup_lane_or_model(name: &str) -> Option<&'static ModelDefaults> {
    REGISTRY
        .iter()
        .find(|m| m.model == name || m.lane_id == name)
}

/// Lane id for a model: the registry slug, else a slug derived from the repo
/// name (`org/Some-Model_v2` -> `some-model-v2`).
pub fn lane_id_for(model_id: &str) -> String {
    if let Some(m) = lookup(model_id) {
        return m.lane_id.to_string();
    }
    let name = model_id.rsplit('/').next().unwrap_or(model_id);
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "dense".to_string()
    } else {
        out
    }
}

/// Default pooling for a family when neither the repo nor the registry says.
pub fn family_default_pooling(family: Family) -> Pooling {
    match family {
        Family::Bert => Pooling::Mean,
        Family::XlmRoberta => Pooling::Cls,
        Family::Qwen3 => Pooling::Last,
    }
}

/// Map a `config.json` `model_type` to a family.
pub fn family_from_model_type(model_type: &str) -> Option<Family> {
    match model_type {
        "bert" => Some(Family::Bert),
        "xlm-roberta" | "xlm_roberta" => Some(Family::XlmRoberta),
        "qwen3" => Some(Family::Qwen3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique() {
        let mut lanes: Vec<&str> = REGISTRY.iter().map(|m| m.lane_id).collect();
        lanes.sort_unstable();
        lanes.dedup();
        assert_eq!(lanes.len(), REGISTRY.len());
        let mut models: Vec<&str> = REGISTRY.iter().map(|m| m.model).collect();
        models.sort_unstable();
        models.dedup();
        assert_eq!(models.len(), REGISTRY.len());
    }

    #[test]
    fn lane_ids_derive_from_unknown_models() {
        assert_eq!(lane_id_for("BAAI/bge-small-en-v1.5"), "bge-small");
        assert_eq!(lane_id_for("org/Some-Model_v2"), "some-model-v2");
        assert_eq!(lane_id_for("///"), "dense");
    }

    #[test]
    fn known_pooling_matches_hub_configs() {
        assert_eq!(
            lookup("sentence-transformers/multi-qa-MiniLM-L6-cos-v1")
                .unwrap()
                .pooling,
            Pooling::Mean
        );
        assert_eq!(
            lookup("BAAI/bge-small-en-v1.5").unwrap().pooling,
            Pooling::Cls
        );
        assert_eq!(
            lookup("Qwen/Qwen3-Embedding-0.6B").unwrap().pooling,
            Pooling::Last
        );
        assert!(matches!(
            lookup("BAAI/bge-m3").unwrap().runtime_spec(),
            RuntimeSpec::WebgpuOnnx { ref repo, .. } if repo == "Xenova/bge-m3"
        ));
        assert_eq!(
            lookup_lane_or_model("qwen3e").unwrap().family,
            Family::Qwen3
        );
    }
}
