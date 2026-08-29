// SPDX-License-Identifier: GPL-3.0-only

//! Eddie CLI: build-time indexer for static site content.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use eddie::chunk::{
    Chunk, ChunkStrategy, Document, chunk_document_with_budget, chunk_document_with_strategy,
    dedupe_chunks, summary_chunk,
};
use eddie::claims::{
    ClaimCorpus, ClaimEntry, apply_claim_edits, build_claim_corpus_from_chunks,
    parse_claim_edits_toml,
};
use eddie::embed::{
    DEFAULT_BATCH_SIZE, DenseEncoder, DenseOverrides, DevicePref, TextKind, device_name,
    load_dense, select_device,
};
use eddie::eval::{
    AcceptanceCase, AcceptanceSuite, evaluate_case, hit_at_k, load_suite, mrr, ndcg_at_k,
    summarize, write_suite,
};
use eddie::index::{DenseLane, IndexBuilder, SCOPE_CHUNKS, SCOPE_CLAIMS, SCOPE_QA, SearchIndex};
use eddie::manifest::{DenseSpec, Quant, RuntimeSpec, SparseSpec, SparseTerm};
use eddie::models;
use eddie::parse::{
    AstroParser, ContentParser, DocusaurusParser, EleventyParser, HugoParser, JekyllParser,
    MkDocsParser, parse_content_dir, parse_content_dir_report,
};
use eddie::qa::{
    OllamaConfig, OpenRouterConfig, QaCorpus, QaEntry, build_qa_corpus_from_chunks,
    build_qa_entries_from_chunks, synthesize_with_ollama_from_chunks,
    synthesize_with_openrouter_from_chunks,
};
use eddie::search::{
    Mode, PageResult, Query, Retrieval, Weights, group_pages, query_terms, retrieve,
};
use eddie::sparse::{
    SparseDocEncoder, SparseOptions, sparse_query_terms, sparse_tokenizer_from_bytes,
    tokenizer_json_sha256,
};

const DEFAULT_MODEL: &str = "sentence-transformers/multi-qa-MiniLM-L6-cos-v1";

#[derive(Parser)]
#[command(name = "eddie", about = "Semantic search indexer for static sites")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Build a search index from a content directory.
    Index {
        /// Path to the content directory (e.g. Hugo's content/).
        #[arg(long)]
        content_dir: PathBuf,

        /// CMS parser profile used to parse content files.
        #[arg(long, default_value = "hugo")]
        cms: Cms,

        /// Output path for the index file.
        #[arg(long, default_value = "index.ed")]
        output: PathBuf,

        /// Dense embedding model: a HuggingFace id or a registry lane id
        /// (`minilm`, `bge-small`, `qwen3e`, `harrier`, `bge-m3`). Repeat for
        /// several lanes. Default: sentence-transformers/multi-qa-MiniLM-L6-cos-v1.
        #[arg(long = "dense-model", value_name = "MODEL")]
        dense_model: Vec<String>,

        /// Deprecated alias for --dense-model.
        #[arg(long, hide = true)]
        model: Option<String>,

        /// Browser runtime for the lane at the same position as --dense-model:
        /// `wasm-candle` or `webgpu-onnx:<repo>:<dtype>[:<dtype_f16>]`.
        #[arg(long = "dense-runtime", value_name = "SPEC")]
        dense_runtime: Vec<String>,

        /// Add the learned-sparse arm (OpenSearch neural sparse doc-v3-distill).
        #[arg(long, default_value_t = false)]
        sparse: bool,

        /// Sparse document encoder model id (implies --sparse).
        #[arg(long, value_name = "MODEL")]
        sparse_model: Option<String>,

        /// Inference device: auto, cpu, cuda or cuda:N (cuda needs `--features cuda`).
        #[arg(long, default_value = "auto")]
        device: String,

        /// Texts per forward pass for dense lanes.
        #[arg(long, default_value_t = DEFAULT_BATCH_SIZE)]
        batch_size: usize,

        /// Model preset: fast (MiniLM), balanced (bge-small + sparse),
        /// quality (bge-small + sparse + Qwen3-Embedding-0.6B), gpu (quality on CUDA).
        /// Explicit --dense-model/--sparse/--device flags win over the preset.
        #[arg(long)]
        preset: Option<Preset>,

        /// Maximum tokens per chunk.
        #[arg(long, default_value = "256")]
        chunk_size: usize,

        /// Overlap tokens between chunks.
        #[arg(long, default_value = "32")]
        overlap: usize,

        /// Chunking strategy: heading-aware (default) or semantic segmentation.
        #[arg(long, default_value = "heading")]
        chunk_strategy: ChunkingStrategy,

        /// Optional coarse chunk size for dual-granularity retrieval.
        #[arg(long)]
        coarse_chunk_size: Option<usize>,

        /// Overlap tokens for coarse chunks (defaults to --overlap).
        #[arg(long)]
        coarse_overlap: Option<usize>,

        /// Add a lightweight summary lane (RAPTOR-style coarse summaries).
        #[arg(long, default_value_t = false)]
        summary_lane: bool,

        /// Include QA entries in the index as an embedded section.
        #[arg(long, default_value_t = false)]
        qa: bool,

        /// Include extracted claims in the index as an embedded section.
        #[arg(long, default_value_t = false)]
        claims: bool,

        /// Also run the regex heuristics that guess QA pairs from prose (off by default; tuned for resume-style pages).
        #[arg(long, default_value_t = false)]
        qa_heuristics: bool,

        /// Also run the regex heuristics that extract claims from prose (off by default).
        #[arg(long, default_value_t = false)]
        claims_heuristics: bool,

        /// Seed passed to the QA synthesis model for reproducible builds.
        #[arg(long)]
        qa_seed: Option<u64>,

        /// Optional claims edits file (TOML with [[add]] / [[redact]]).
        #[arg(long)]
        claims_edits: Option<PathBuf>,

        /// Optional Ollama model for QA synthesis at index time.
        #[arg(long)]
        qa_ollama_model: Option<String>,

        /// Optional OpenRouter model for QA synthesis at index time.
        #[arg(long)]
        qa_openrouter_model: Option<String>,

        /// OpenRouter chat-completions endpoint.
        #[arg(long, default_value = "https://openrouter.ai/api/v1/chat/completions")]
        qa_openrouter_url: String,

        /// Environment variable name for OpenRouter API key.
        #[arg(long, default_value = "OPENROUTER_API_KEY")]
        qa_openrouter_api_key_env: String,

        /// Ollama generate endpoint for QA synthesis.
        #[arg(long, default_value = "http://127.0.0.1:11434/api/generate")]
        qa_ollama_url: String,

        /// Max chunks to send to Ollama during QA synthesis.
        #[arg(long, default_value = "48")]
        qa_ollama_max_chunks: usize,

        /// Max QA pairs per chunk from Ollama.
        #[arg(long, default_value = "3")]
        qa_ollama_max_pairs_per_chunk: usize,

        /// Sampling temperature for Ollama QA synthesis.
        #[arg(long, default_value = "0.2")]
        qa_ollama_temperature: f32,
    },

    /// Search an existing index.
    Search {
        /// Path to the index file.
        #[arg(long)]
        index: PathBuf,

        /// Search query.
        #[arg(long)]
        query: String,

        /// Number of pages to return.
        #[arg(long, default_value = "8")]
        top_k: usize,

        /// Search mode: hybrid (default), dense, sparse, or keyword.
        #[arg(long, default_value = "hybrid")]
        mode: SearchMode,

        /// Dense lane id to embed the query with (default: the index's first lane).
        #[arg(long)]
        lane: Option<String>,

        /// Print the result set as JSON instead of a table.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Print the manifest and section sizes of an index.
    Stats {
        /// Path to the index file.
        #[arg(long)]
        index: PathBuf,

        /// Print as JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Score an index against a labelled query set (Hit@k, MRR, nDCG@k).
    Eval {
        /// Path to the index file.
        #[arg(long)]
        index: PathBuf,

        /// TOML file with `[[cases]] query = "..." relevant = ["/url/", ...]`.
        #[arg(long)]
        labels: PathBuf,

        /// Cut-off k for Hit@k and nDCG@k.
        #[arg(long, default_value = "10")]
        top_k: usize,

        /// Search mode to evaluate.
        #[arg(long, default_value = "hybrid")]
        mode: SearchMode,

        /// Dense lane id to embed the queries with (default: the index's first lane).
        #[arg(long)]
        lane: Option<String>,

        /// Print the per-query report as JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Tune chunking parameters against a site-owned acceptance suite.
    Tune {
        /// Path to content directory.
        #[arg(long)]
        content_dir: PathBuf,

        /// CMS parser profile used to parse content files.
        #[arg(long, default_value = "hugo")]
        cms: Cms,

        /// Path to acceptance JSON suite.
        #[arg(long)]
        eval: Option<PathBuf>,

        /// Persist acceptance suite (useful with --interactive).
        #[arg(long)]
        save_eval: Option<PathBuf>,

        /// Enable an interactive feedback loop to add/score cases.
        #[arg(long, default_value_t = false)]
        interactive: bool,

        /// HuggingFace model ID for embeddings.
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

        /// Comma-separated chunk sizes to test, e.g. 192,256,320.
        #[arg(long, default_value = "192,256,320")]
        chunk_sizes: String,

        /// Comma-separated overlaps to test, e.g. 16,32,48.
        #[arg(long, default_value = "16,32,48")]
        overlaps: String,

        /// Top-k retrieval depth per case (unless case.top_k overrides).
        #[arg(long, default_value = "5")]
        top_k: usize,

        /// Search mode to tune for.
        #[arg(long, default_value = "hybrid")]
        mode: SearchMode,

        /// Optional JSON report output path.
        #[arg(long)]
        report: Option<PathBuf>,
    },

    /// Build a factual Q&A corpus from an existing search index.
    QaCorpus {
        /// Path to index.ed/index.bin input.
        #[arg(long)]
        index: PathBuf,

        /// Output JSON path for Q&A corpus.
        #[arg(long, default_value = "qa-corpus.json")]
        output: PathBuf,

        /// Optional Ollama model for synthesis pass (e.g. qwen2.5:7b-instruct).
        #[arg(long)]
        ollama_model: Option<String>,

        /// Ollama generate endpoint.
        #[arg(long, default_value = "http://127.0.0.1:11434/api/generate")]
        ollama_url: String,

        /// Max fact-dense chunks to send to Ollama.
        #[arg(long, default_value = "48")]
        ollama_max_chunks: usize,

        /// Max QA pairs to request per chunk from Ollama.
        #[arg(long, default_value = "3")]
        ollama_max_pairs_per_chunk: usize,

        /// Sampling temperature for Ollama synthesis.
        #[arg(long, default_value = "0.2")]
        ollama_temperature: f32,
    },

    /// Build a factual claims corpus from an existing search index.
    ClaimsCorpus {
        /// Path to index.ed/index.bin input.
        #[arg(long)]
        index: PathBuf,

        /// Output JSON path for claims corpus.
        #[arg(long, default_value = "claims-corpus.json")]
        output: PathBuf,

        /// Optional claims edits file to apply.
        #[arg(long)]
        claims_edits: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Preset {
    Fast,
    Balanced,
    Quality,
    Gpu,
}

/// Dense lanes, sparse arm and device for `eddie index`, after presets and
/// flags are reconciled.
#[derive(Debug, Clone, PartialEq)]
struct IndexModelOptions {
    dense_models: Vec<String>,
    dense_runtimes: Vec<Option<RuntimeSpec>>,
    sparse_model: Option<String>,
    device: DevicePref,
    batch_size: usize,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SearchMode {
    Hybrid,
    Dense,
    Sparse,
    Keyword,
}

impl From<SearchMode> for Mode {
    fn from(m: SearchMode) -> Self {
        match m {
            SearchMode::Hybrid => Mode::Hybrid,
            SearchMode::Dense => Mode::Dense,
            SearchMode::Sparse => Mode::Sparse,
            SearchMode::Keyword => Mode::Keyword,
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ChunkingStrategy {
    Heading,
    Semantic,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Cms {
    Hugo,
    Astro,
    Docusaurus,
    Mkdocs,
    Eleventy,
    Jekyll,
}

impl Cms {
    fn as_str(self) -> &'static str {
        match self {
            Cms::Hugo => "hugo",
            Cms::Astro => "astro",
            Cms::Docusaurus => "docusaurus",
            Cms::Mkdocs => "mkdocs",
            Cms::Eleventy => "eleventy",
            Cms::Jekyll => "jekyll",
        }
    }
}

#[derive(serde::Serialize)]
struct TuneCandidate {
    chunk_size: usize,
    overlap: usize,
    passed_cases: usize,
    total_cases: usize,
    pass_rate: f32,
    weighted_score: f32,
    weighted_total: f32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Index {
            content_dir,
            cms,
            output,
            dense_model,
            model,
            dense_runtime,
            sparse,
            sparse_model,
            device,
            batch_size,
            preset,
            chunk_size,
            overlap,
            chunk_strategy,
            coarse_chunk_size,
            coarse_overlap,
            summary_lane,
            qa,
            claims,
            qa_heuristics,
            claims_heuristics,
            qa_seed,
            claims_edits,
            qa_ollama_model,
            qa_openrouter_model,
            qa_openrouter_url,
            qa_openrouter_api_key_env,
            qa_ollama_url,
            qa_ollama_max_chunks,
            qa_ollama_max_pairs_per_chunk,
            qa_ollama_temperature,
        } => cmd_index(
            content_dir,
            cms,
            output,
            &resolve_index_models(
                preset,
                dense_model,
                model,
                &dense_runtime,
                sparse,
                sparse_model,
                &device,
                batch_size,
            )?,
            chunk_size,
            overlap,
            chunk_strategy,
            coarse_chunk_size,
            coarse_overlap,
            summary_lane,
            qa,
            claims,
            qa_heuristics,
            claims_heuristics,
            qa_seed,
            claims_edits,
            qa_ollama_model,
            qa_openrouter_model,
            qa_openrouter_url,
            qa_openrouter_api_key_env,
            qa_ollama_url,
            qa_ollama_max_chunks,
            qa_ollama_max_pairs_per_chunk,
            qa_ollama_temperature,
        ),
        Command::Search {
            index,
            query,
            top_k,
            mode,
            lane,
            json,
        } => cmd_search(index, &query, top_k, mode.into(), lane.as_deref(), json),
        Command::Stats { index, json } => cmd_stats(index, json),
        Command::Eval {
            index,
            labels,
            top_k,
            mode,
            lane,
            json,
        } => cmd_eval(index, labels, top_k, mode.into(), lane.as_deref(), json),
        Command::Tune {
            content_dir,
            cms,
            eval,
            save_eval,
            interactive,
            model,
            chunk_sizes,
            overlaps,
            top_k,
            mode,
            report,
        } => cmd_tune(
            content_dir,
            cms,
            eval,
            save_eval,
            interactive,
            &model,
            &chunk_sizes,
            &overlaps,
            top_k,
            mode,
            report,
        ),
        Command::QaCorpus {
            index,
            output,
            ollama_model,
            ollama_url,
            ollama_max_chunks,
            ollama_max_pairs_per_chunk,
            ollama_temperature,
        } => cmd_qa_corpus(
            index,
            output,
            ollama_model,
            ollama_url,
            ollama_max_chunks,
            ollama_max_pairs_per_chunk,
            ollama_temperature,
        ),
        Command::ClaimsCorpus {
            index,
            output,
            claims_edits,
        } => cmd_claims_corpus(index, output, claims_edits),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_index(
    content_dir: PathBuf,
    cms: Cms,
    output: PathBuf,
    model_opts: &IndexModelOptions,
    chunk_size: usize,
    overlap: usize,
    chunk_strategy: ChunkingStrategy,
    coarse_chunk_size: Option<usize>,
    coarse_overlap: Option<usize>,
    summary_lane: bool,
    qa_enabled: bool,
    claims_enabled: bool,
    qa_heuristics: bool,
    claims_heuristics: bool,
    qa_seed: Option<u64>,
    claims_edits_path: Option<PathBuf>,
    qa_ollama_model: Option<String>,
    qa_openrouter_model: Option<String>,
    qa_openrouter_url: String,
    qa_openrouter_api_key_env: String,
    qa_ollama_url: String,
    qa_ollama_max_chunks: usize,
    qa_ollama_max_pairs_per_chunk: usize,
    qa_ollama_temperature: f32,
) -> Result<()> {
    // Parse content
    eprintln!(
        "Parsing content from {} with {} parser...",
        content_dir.display(),
        cms.as_str()
    );
    let parser = parser_for(cms);
    let report = parse_content_dir_report(&content_dir, parser.as_ref(), false)?;
    let docs = report.docs;
    eprintln!(
        "  Found {} documents ({} files skipped)",
        docs.len(),
        report.skipped.len()
    );
    if docs.is_empty() {
        bail!("no documents found under {}", content_dir.display());
    }

    // Load models first: the first dense lane's tokenizer is the token
    // counter for chunk sizing once the chunker takes one.
    let device = select_device(model_opts.device)?;
    eprintln!("Inference device: {}", device_name(&device));
    let mut lanes: Vec<Box<dyn DenseEncoder>> = Vec::with_capacity(model_opts.dense_models.len());
    for (i, model_id) in model_opts.dense_models.iter().enumerate() {
        eprintln!("Loading dense model: {}...", model_id);
        let started = Instant::now();
        let overrides = DenseOverrides {
            runtime: model_opts.dense_runtimes.get(i).cloned().flatten(),
            batch_size: Some(model_opts.batch_size),
            ..DenseOverrides::default()
        };
        let lane = load_dense(model_id, &device, &overrides)?;
        let spec = lane.spec();
        if lanes.iter().any(|l| l.spec().id == spec.id) {
            bail!("dense lane id '{}' is used twice", spec.id);
        }
        eprintln!(
            "  lane '{}': family {:?}, dim {}, pooling {:?}, max_seq_len {}, revision {} ({:.1}s)",
            spec.id,
            spec.family,
            spec.dim,
            spec.pooling,
            spec.max_seq_len,
            spec.revision.as_deref().unwrap_or("main"),
            started.elapsed().as_secs_f64()
        );
        lanes.push(lane);
    }
    if lanes.is_empty() {
        bail!("at least one --dense-model is required");
    }
    let sparse_encoder = match &model_opts.sparse_model {
        Some(model_id) => {
            eprintln!("Loading sparse encoder: {}...", model_id);
            let started = Instant::now();
            let enc = SparseDocEncoder::load_with(model_id, &device, &SparseOptions::default())?;
            eprintln!(
                "  {} idf terms, activation {:?}, prune ratio {}, revision {} ({:.1}s)",
                enc.idf().len(),
                enc.activation(),
                enc.prune_ratio(),
                enc.revision().unwrap_or("main"),
                started.elapsed().as_secs_f64()
            );
            Some(enc)
        }
        None => None,
    };

    // Chunk documents, sized in the first dense lane's tokens so nothing is
    // silently truncated by the embedder.
    let strategy = match chunk_strategy {
        ChunkingStrategy::Heading => ChunkStrategy::Heading,
        ChunkingStrategy::Semantic => ChunkStrategy::Semantic,
    };
    let primary = lanes[0].as_ref();
    let budget = chunk_size.min(primary.spec().max_seq_len);
    let count = |text: &str| primary.count_tokens(text);
    eprintln!(
        "Chunking documents (strategy: {:?}, budget {} tokens, overlap {} tokens, counted with lane '{}')...",
        chunk_strategy,
        budget,
        overlap,
        primary.spec().id
    );
    let mut all_chunks = Vec::new();
    for doc in &docs {
        let mut fine = chunk_document_with_budget(doc, budget, overlap, strategy, &count);
        for chunk in &mut fine {
            chunk.meta.granularity = Some("fine".to_string());
        }
        all_chunks.extend(fine);

        if let Some(coarse_size) = coarse_chunk_size {
            let coarse_overlap = coarse_overlap.unwrap_or(overlap);
            let coarse_budget = coarse_size.min(primary.spec().max_seq_len);
            let mut coarse =
                chunk_document_with_budget(doc, coarse_budget, coarse_overlap, strategy, &count);
            for chunk in &mut coarse {
                chunk.meta.granularity = Some("coarse".to_string());
            }
            all_chunks.extend(coarse);
        }

        if summary_lane && let Some(mut summary) = summary_chunk(doc) {
            summary.meta.granularity = Some("summary".to_string());
            all_chunks.push(summary);
        }
    }
    let before_dedupe = all_chunks.len();
    let all_chunks = dedupe_chunks(all_chunks);
    eprintln!(
        "  Created {} chunks ({} duplicate chunks dropped)",
        all_chunks.len(),
        before_dedupe - all_chunks.len()
    );

    // Keep factual extraction stable even when retrieval chunking is semantic/coarse.
    let fact_chunks: Vec<Chunk> = if qa_enabled || claims_enabled {
        docs.iter()
            .flat_map(|doc| {
                chunk_document_with_budget(doc, budget, overlap, ChunkStrategy::Heading, &count)
            })
            .collect()
    } else {
        Vec::new()
    };
    let fact_metadata: Vec<_> = fact_chunks.iter().map(|c| c.meta.clone()).collect();
    let fact_texts: Vec<String> = fact_chunks.iter().map(|c| c.text.clone()).collect();

    // Embed all chunks (overlap prefix + text), one pass per dense lane.
    // BM25, the sparse arm and the stored texts use the clean text only.
    let embed_inputs: Vec<String> = all_chunks.iter().map(|c| c.embed_text()).collect();
    let embed_refs: Vec<&str> = embed_inputs.iter().map(String::as_str).collect();
    let texts: Vec<&str> = all_chunks.iter().map(|c| c.text.as_str()).collect();
    let mut lane_vectors: Vec<Vec<f32>> = Vec::with_capacity(lanes.len());
    for lane in &lanes {
        eprintln!(
            "Embedding {} chunks with lane '{}'...",
            all_chunks.len(),
            lane.spec().id
        );
        lane.reset_truncated_count();
        let started = Instant::now();
        let vectors = embed_texts_with(
            lane.as_ref(),
            &embed_refs,
            TextKind::Document,
            model_opts.batch_size,
        )?;
        report_lane_timing(
            lane.as_ref(),
            all_chunks.len(),
            started.elapsed().as_secs_f64(),
        );
        lane_vectors.push(vectors);
    }

    // Learned-sparse document expansion
    let mut sparse_section: Option<(Vec<Vec<SparseTerm>>, SparseSpec)> = None;
    if let Some(enc) = &sparse_encoder {
        eprintln!(
            "Expanding {} chunks with the sparse encoder...",
            texts.len()
        );
        let started = Instant::now();
        let sparse_docs = enc.encode_docs(&texts)?;
        let total_terms: usize = sparse_docs.iter().map(Vec::len).sum();
        let distinct: BTreeSet<u32> = sparse_docs
            .iter()
            .flat_map(|d| d.iter().map(|t| t.token_id))
            .collect();
        let secs = started.elapsed().as_secs_f64();
        eprintln!(
            "  {} chunks in {:.1}s ({:.1} chunks/s): {} distinct terms, {:.1} terms/chunk, {} truncated at 512 tokens",
            texts.len(),
            secs,
            texts.len() as f64 / secs.max(1e-9),
            distinct.len(),
            total_terms as f64 / texts.len().max(1) as f64,
            enc.truncated_count()
        );
        sparse_section = Some((sparse_docs, enc.spec(distinct.len())));
    }

    // Build optional QA/claims sections
    let metadata: Vec<_> = all_chunks.iter().map(|c| c.meta.clone()).collect();
    let chunk_texts: Vec<String> = all_chunks.iter().map(|c| c.text.clone()).collect();
    let mut qa_entries: Vec<QaEntry> = Vec::new();
    let mut claims: Vec<ClaimEntry> = Vec::new();

    if qa_enabled {
        eprintln!("Building QA section...");
        if qa_heuristics {
            qa_entries = build_qa_entries_from_chunks(&fact_texts, &fact_metadata);
            eprintln!("  Heuristic QA entries: {}", qa_entries.len());
        }
        if let Some(model) = qa_openrouter_model {
            let api_key = std::env::var(&qa_openrouter_api_key_env).with_context(|| {
                format!(
                    "reading OpenRouter API key from env var {}",
                    qa_openrouter_api_key_env
                )
            })?;
            let cfg = OpenRouterConfig {
                model,
                endpoint: qa_openrouter_url,
                api_key,
                max_chunks: qa_ollama_max_chunks,
                max_pairs_per_chunk: qa_ollama_max_pairs_per_chunk,
                temperature: qa_ollama_temperature,
                seed: qa_seed,
                ..Default::default()
            };
            eprintln!("  Running OpenRouter QA synthesis...");
            let llm_entries =
                synthesize_with_openrouter_from_chunks(&fact_texts, &fact_metadata, &cfg)?;
            eprintln!("  OpenRouter QA entries: {}", llm_entries.len());
            qa_entries.extend(llm_entries);
        } else if let Some(model) = qa_ollama_model {
            let cfg = OllamaConfig {
                model,
                endpoint: qa_ollama_url,
                max_chunks: qa_ollama_max_chunks,
                max_pairs_per_chunk: qa_ollama_max_pairs_per_chunk,
                temperature: qa_ollama_temperature,
                seed: qa_seed,
                ..Default::default()
            };
            eprintln!("  Running Ollama QA synthesis...");
            let llm_entries =
                synthesize_with_ollama_from_chunks(&fact_texts, &fact_metadata, &cfg)?;
            eprintln!("  Ollama QA entries: {}", llm_entries.len());
            qa_entries.extend(llm_entries);
        } else if !qa_heuristics {
            eprintln!(
                "  warning: --qa without an LLM model and without --qa-heuristics produces no entries"
            );
        }
        let mut corpus = QaCorpus {
            version: 1,
            entries: qa_entries,
        };
        corpus.dedup();
        qa_entries = corpus.entries;
        eprintln!("  QA entries: {}", qa_entries.len());
    }

    if claims_enabled {
        eprintln!("Building claims section...");
        let mut corpus = if claims_heuristics {
            build_claim_corpus_from_chunks(&fact_texts, &fact_metadata)
        } else {
            ClaimCorpus {
                version: 1,
                claims: Vec::new(),
            }
        };
        if let Some(path) = claims_edits_path {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading claims edits {}", path.display()))?;
            let edits = parse_claim_edits_toml(&raw)?;
            apply_claim_edits(&mut corpus.claims, &edits);
            eprintln!(
                "  Applied claims edits from {} (now {} claims)",
                path.display(),
                corpus.claims.len()
            );
        }
        claims = corpus.claims;
        eprintln!("  Claims entries: {}", claims.len());
    }

    // --- Index assembly (format v5) -------------------------------------
    let n = metadata.len();
    // Stored texts are the clean chunk text (no overlap prefix), so the
    // builder has nothing to strip.
    let overlap_words: Vec<u16> = vec![0; n];

    let mut builder = IndexBuilder::new();
    builder.add_chunks(metadata, chunk_texts, overlap_words)?;
    for (lane, vectors) in lanes.iter().zip(lane_vectors) {
        let dim = lane.dim();
        builder.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(lane.spec().clone(), dim, n, &vectors, Quant::Int8)?,
        )?;
    }
    if let (Some(enc), Some((sparse_docs, sparse_spec))) = (&sparse_encoder, sparse_section) {
        builder.add_sparse(&sparse_docs, enc.idf(), sparse_spec)?;
    }

    if !qa_entries.is_empty() {
        eprintln!("Embedding QA section ({} entries)...", qa_entries.len());
        let qa_texts: Vec<String> = qa_entries
            .iter()
            .map(|q| format!("Q: {} A: {}", q.question, q.answer))
            .collect();
        let refs: Vec<&str> = qa_texts.iter().map(String::as_str).collect();
        for lane in &lanes {
            let vectors = embed_texts_with(
                lane.as_ref(),
                &refs,
                TextKind::Document,
                model_opts.batch_size,
            )?;
            builder.add_dense_lane(
                SCOPE_QA,
                DenseLane::from_f32(
                    lane.spec().clone(),
                    lane.dim(),
                    qa_entries.len(),
                    &vectors,
                    Quant::Int8,
                )?,
            )?;
        }
        builder.add_qa(qa_entries);
    }

    if !claims.is_empty() {
        eprintln!("Embedding claims section ({} claims)...", claims.len());
        let claim_texts: Vec<String> = claims
            .iter()
            .map(|c| format!("{} {} {} {}", c.subject, c.predicate, c.object, c.evidence))
            .collect();
        let refs: Vec<&str> = claim_texts.iter().map(String::as_str).collect();
        for lane in &lanes {
            let vectors = embed_texts_with(
                lane.as_ref(),
                &refs,
                TextKind::Document,
                model_opts.batch_size,
            )?;
            builder.add_dense_lane(
                SCOPE_CLAIMS,
                DenseLane::from_f32(
                    lane.spec().clone(),
                    lane.dim(),
                    claims.len(),
                    &vectors,
                    Quant::Int8,
                )?,
            )?;
        }
        builder.add_claims(claims);
    }

    let index = builder.finish()?;

    eprintln!("Writing index to {}...", output.display());
    let file = fs::File::create(&output)
        .with_context(|| format!("creating output file {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    index.write_ed_to(&mut writer)?;
    writer.flush()?;

    eprintln!(
        "Done! Index contains {} chunks over {} pages, {} qa entries, {} claims; lanes: {}.",
        index.manifest.chunks,
        index.manifest.pages,
        index.qa.len(),
        index.claims.len(),
        index
            .manifest
            .dense
            .iter()
            .map(|d| format!("{} ({}-d {:?})", d.id, d.dim, d.quant))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// Embeds queries with one of the index's own dense lanes.
struct QueryEmbedder {
    lane: usize,
    spec: DenseSpec,
    embedder: Box<dyn DenseEncoder>,
}

impl QueryEmbedder {
    /// Pick `lane_id` (default: the index's first lane) and load its model
    /// natively. Every family the indexer can build (bert, xlm-roberta,
    /// qwen3) can embed queries here, whatever browser runtime the lane
    /// declares; the lane's own pooling, prefixes and sequence cap are
    /// applied so query vectors match the stored document vectors.
    fn for_index(index: &SearchIndex, lane_id: Option<&str>) -> Result<Self> {
        if index.manifest.dense.is_empty() {
            bail!("index has no dense lane; use --mode keyword or --mode sparse");
        }
        let spec = match lane_id {
            Some(id) => index
                .manifest
                .dense_lane(id)
                .with_context(|| {
                    format!(
                        "lane {:?} is not in the index (lanes: {})",
                        id,
                        lane_list(index)
                    )
                })?
                .clone(),
            None => index.manifest.dense[0].clone(),
        };
        let lane = index
            .dense_lane(&spec.id)
            .with_context(|| format!("index has no dense section for lane {:?}", spec.id))?;
        eprintln!(
            "Loading embedding model for lane {} ({:?}, {}): {}...",
            spec.id,
            spec.family,
            runtime_kind(&spec.runtime),
            spec.model
        );
        let device = select_device(DevicePref::Auto)?;
        let overrides = DenseOverrides {
            lane_id: Some(spec.id.clone()),
            revision: spec.revision.clone(),
            batch_size: None,
        };
        let embedder = load_dense(&spec.model, &device, &overrides)
            .with_context(|| format!("loading the model for lane {:?}", spec.id))?;
        if embedder.dim() != spec.dim {
            bail!(
                "model {} produces {}-d vectors but lane {:?} stores {}-d",
                spec.model,
                embedder.dim(),
                spec.id,
                spec.dim
            );
        }
        Ok(Self {
            lane,
            spec,
            embedder,
        })
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        embed_query(self.embedder.as_ref(), text)
    }
}

/// One query embedding; the encoder applies the lane's query prefix.
fn embed_query(encoder: &dyn DenseEncoder, text: &str) -> Result<Vec<f32>> {
    let mut vecs = encoder.embed(&[text], TextKind::Query)?;
    vecs.pop().context("embedder returned no vector")
}

fn lane_list(index: &SearchIndex) -> String {
    index
        .manifest
        .dense
        .iter()
        .map(|d| format!("{} [{}]", d.id, runtime_kind(&d.runtime)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fetch the sparse arm's `tokenizer.json` from HuggingFace (pinned to the
/// manifest revision), verify its SHA-256 against the manifest `vocab_hash`
/// and prepare it like the document side (no padding, truncation at 512).
/// Returns `None`, with a warning, when it cannot be loaded or does not
/// match, so the search degrades instead of failing or scoring against the
/// wrong vocabulary.
fn load_sparse_tokenizer(index: &SearchIndex) -> Option<tokenizers::Tokenizer> {
    let spec = index.manifest.sparse.as_ref()?;
    let fetch = || -> Result<tokenizers::Tokenizer> {
        let repo = eddie::embed::hub::ModelRepo::open(&spec.tokenizer, spec.revision.as_deref())
            .with_context(|| format!("opening HuggingFace repo {}", spec.tokenizer))?;
        let path = repo
            .get("tokenizer.json")
            .with_context(|| format!("downloading tokenizer.json from {}", spec.tokenizer))?;
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let actual = tokenizer_json_sha256(&bytes);
        if !actual.eq_ignore_ascii_case(&spec.vocab_hash) {
            bail!(
                "tokenizer.json from {} (revision {}) has sha256 {} but the index was built with vocab_hash {}; its token ids would not match the sparse postings",
                spec.tokenizer,
                spec.revision.as_deref().unwrap_or("main"),
                actual,
                spec.vocab_hash
            );
            family: Some(spec.family),
            pooling: Some(spec.pooling),
            max_seq_len: Some(spec.max_seq_len),
            query_prefix: Some(spec.query_prefix.clone()),
            doc_prefix: Some(spec.doc_prefix.clone()),
            normalize: Some(spec.normalize),
            runtime: Some(spec.runtime.clone()),
        }
        sparse_tokenizer_from_bytes(&bytes, eddie::sparse::DEFAULT_MAX_SEQ_LEN)
    };
    match fetch() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("warning: sparse arm skipped: {:#}", e);
            None
        }
    }
}

/// Everything the CLI needs to run queries against an index the way the
/// widget does.
struct QueryRuntime {
    dense: Option<QueryEmbedder>,
    sparse_tokenizer: Option<tokenizers::Tokenizer>,
    mode: Mode,
}

impl QueryRuntime {
    fn new(index: &SearchIndex, mode: Mode, lane: Option<&str>) -> Result<Self> {
        let dense = match mode {
            Mode::Hybrid if index.manifest.dense.is_empty() => {
                eprintln!("warning: index has no dense lane; hybrid runs without the dense arm");
                None
            }
            Mode::Hybrid | Mode::Dense => Some(QueryEmbedder::for_index(index, lane)?),
            Mode::Sparse | Mode::Keyword => None,
        };
        let sparse_tokenizer =
            if matches!(mode, Mode::Hybrid | Mode::Sparse) && index.sparse.is_some() {
                load_sparse_tokenizer(index)
            } else {
fn runtime_kind(runtime: &RuntimeSpec) -> &'static str {
    match runtime {
        RuntimeSpec::WasmCandle { .. } => "wasm-candle",
        RuntimeSpec::WebgpuOnnx { .. } => "webgpu-onnx",
    }
}

                None
            };
        Ok(Self {
            dense,
            sparse_tokenizer,
            mode,
        })
    }

    fn sparse_terms(&self, index: &SearchIndex, text: &str) -> Result<Option<Vec<SparseTerm>>> {
        match (&index.sparse, &self.sparse_tokenizer) {
            (Some(sparse), Some(tok)) => {
                Ok(Some(sparse_query_terms(tok, &|id| sparse.idf_of(id), text)))
            }
            _ => Ok(None),
        }
    }

    /// Retrieve and group pages exactly like the widget.
    fn run(
        &self,
        index: &SearchIndex,
        text: &str,
        top_k: usize,
    ) -> Result<(Vec<PageResult>, Retrieval)> {
        let dense = match &self.dense {
            Some(e) => Some((e.lane, e.embed(text)?)),
            None => None,
        };
        let q = Query {
            text,
            dense,
            sparse: self.sparse_terms(index, text)?,
            mode: self.mode,
            top_k,
            weights: Weights::default(),
        };
        let retrieval = retrieve(index, &q)?;
        let pages = group_pages(index, &retrieval.ranked, &query_terms(text), top_k);
        Ok((pages, retrieval))
    }
}

fn load_index(path: &PathBuf) -> Result<SearchIndex> {
    eprintln!("Loading index from {}...", path.display());
    let bytes = fs::read(path).with_context(|| format!("opening index file {}", path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;
    eprintln!(
        "  {} chunks, {} pages, lanes: {}, sparse terms: {}",
        index.manifest.chunks,
        index.manifest.pages,
        if index.manifest.dense.is_empty() {
            "none".to_string()
        } else {
            lane_list(&index)
        },
        index.sparse.as_ref().map(|s| s.term_count()).unwrap_or(0)
    );
    Ok(index)
}

#[derive(serde::Serialize)]
struct SearchOutput<'a> {
    query: &'a str,
    mode: Mode,
    dense_lane: Option<&'a str>,
    arms: eddie::search::Arms,
    degraded: &'a [String],
    results: &'a [PageResult],
}

fn cmd_search(
    index_path: PathBuf,
    query: &str,
    top_k: usize,
    mode: Mode,
    lane: Option<&str>,
    json: bool,
) -> Result<()> {
    if top_k == 0 {
        bail!("--top-k must be > 0");
    }
    let index = load_index(&index_path)?;
    let runtime = QueryRuntime::new(&index, mode, lane)?;
    let (pages, retrieval) = runtime.run(&index, query, top_k)?;

    if json {
        let out = SearchOutput {
            query,
            mode,
            dense_lane: runtime.dense.as_ref().map(|d| d.spec.id.as_str()),
            arms: retrieval.arms,
            degraded: &retrieval.degraded,
            results: &pages,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!(
        "\n{} results for: \"{}\"  (arms: dense={} sparse={} bm25={})",
        mode.as_str(),
        query,
        retrieval.arms.dense,
        retrieval.arms.sparse,
        retrieval.arms.bm25
    );
    for note in &retrieval.degraded {
        println!("  note: {}", note);
    }
    println!("{}", "-".repeat(60));
    if pages.is_empty() {
        println!("(no results)");
    }
    for (rank, page) in pages.iter().enumerate() {
        println!(
            "{}. [{:.4}] {} — {}",
            rank + 1,
            page.score,
            page.title,
            page.url
        );
        if let Some(section) = &page.section {
            println!("   Section: {}", section);
        }
        let ranks = retrieval
            .ranked
            .iter()
            .find(|c| c.chunk == page.chunk)
            .map(|c| {
                format!(
                    "chunk {} (dense {} / sparse {} / bm25 {}), {} chunk(s) on page",
                    c.chunk,
                    c.dense_rank.map_or("-".to_string(), |r| r.to_string()),
                    c.sparse_rank.map_or("-".to_string(), |r| r.to_string()),
                    c.bm25_rank.map_or("-".to_string(), |r| r.to_string()),
                    page.chunks.len()
                )
            })
            .unwrap_or_default();
        println!("   {}", ranks);
        println!("   {}", page.snippet);
    }
    Ok(())
}

fn cmd_stats(index_path: PathBuf, json: bool) -> Result<()> {
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let info = SearchIndex::inspect(&bytes, Some(9))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    println!("Manifest:");
    println!("{}", serde_json::to_string_pretty(&info.manifest)?);
    println!();
    println!(
        "File: {} bytes = header {} + manifest {} + compressed payload {} (raw payload {})",
        info.file_bytes,
        info.file_bytes - info.manifest_bytes - info.payload_compressed_bytes,
        info.manifest_bytes,
        info.payload_compressed_bytes,
        info.payload_bytes
    );
    println!();
    println!(
        "{:<28} {:>12} {:>14}",
        "section", "raw bytes", "brotli (est.)"
    );
    println!("{}", "-".repeat(56));
    for s in &info.sections {
        println!(
            "{:<28} {:>12} {:>14}",
            s.name,
            s.raw_bytes,
            s.compressed_bytes
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    println!();
    for lane in &info.manifest.dense {
        let kind = match &lane.runtime {
            RuntimeSpec::WasmCandle { .. } => "wasm-candle".to_string(),
            RuntimeSpec::WebgpuOnnx { repo, dtype, .. } => {
                format!("webgpu-onnx {} {}", repo, dtype)
            }
        };
        println!(
            "lane {:<12} {}  {}-d {:?} pooling={:?}  [{}]",
            lane.id, lane.model, lane.dim, lane.quant, lane.pooling, kind
        );
    }
    match &info.manifest.sparse {
        Some(s) => println!(
            "sparse: {} terms, tokenizer {} (vocab {})",
            s.terms, s.tokenizer, s.vocab_hash
        ),
        None => println!("sparse: none"),
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct LabelSet {
    cases: Vec<LabelCase>,
}

#[derive(Debug, serde::Deserialize)]
struct LabelCase {
    #[serde(default)]
    id: Option<String>,
    query: String,
    /// Relevant page URLs.
    relevant: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct CaseMetrics {
    id: String,
    query: String,
    hit: f64,
    rr: f64,
    ndcg: f64,
    first_relevant_rank: Option<usize>,
    top: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct EvalReport {
    k: usize,
    mode: Mode,
    cases: usize,
    hit_at_k: f64,
    mrr: f64,
    ndcg_at_k: f64,
    per_case: Vec<CaseMetrics>,
}

fn cmd_eval(
    index_path: PathBuf,
    labels: PathBuf,
    top_k: usize,
    mode: Mode,
    lane: Option<&str>,
    json: bool,
) -> Result<()> {
    if top_k == 0 {
        bail!("--top-k must be > 0");
    }
    let raw = fs::read_to_string(&labels)
        .with_context(|| format!("reading labels {}", labels.display()))?;
    let set: LabelSet = toml::from_str(&raw)
        .with_context(|| format!("parsing labels {} as TOML", labels.display()))?;
    if set.cases.is_empty() {
        bail!("labels file has no [[cases]]");
    }
    for (i, c) in set.cases.iter().enumerate() {
        if c.query.trim().is_empty() || c.relevant.is_empty() {
            bail!("case {} needs a query and at least one relevant url", i + 1);
        }
    }

    let index = load_index(&index_path)?;
    let runtime = QueryRuntime::new(&index, mode, lane)?;

    let mut per_case = Vec::with_capacity(set.cases.len());
    for (i, case) in set.cases.iter().enumerate() {
        let (pages, _) = runtime.run(&index, &case.query, top_k)?;
        let urls: Vec<String> = pages.into_iter().map(|p| p.url).collect();
        per_case.push(CaseMetrics {
            id: case.id.clone().unwrap_or_else(|| format!("case-{}", i + 1)),
            query: case.query.clone(),
            hit: hit_at_k(&urls, &case.relevant, top_k),
            rr: mrr(&urls, &case.relevant),
            ndcg: ndcg_at_k(&urls, &case.relevant, top_k),
            first_relevant_rank: urls
                .iter()
                .position(|u| case.relevant.iter().any(|r| r == u))
                .map(|p| p + 1),
            top: urls,
        });
    }
    let n = per_case.len() as f64;
    let report = EvalReport {
        k: top_k,
        mode,
        cases: per_case.len(),
        hit_at_k: per_case.iter().map(|c| c.hit).sum::<f64>() / n,
        mrr: per_case.iter().map(|c| c.rr).sum::<f64>() / n,
        ndcg_at_k: per_case.iter().map(|c| c.ndcg).sum::<f64>() / n,
        per_case,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "\n{:<24} {:>6} {:>6} {:>6}  first relevant",
        "case", "hit", "rr", "ndcg"
    );
    println!("{}", "-".repeat(60));
    for c in &report.per_case {
        println!(
            "{:<24} {:>6.2} {:>6.2} {:>6.2}  {}",
            truncate_label(&c.id, 24),
            c.hit,
            c.rr,
            c.ndcg,
            c.first_relevant_rank
                .map(|r| r.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    println!("{}", "-".repeat(60));
    println!(
        "{} cases, mode {}: Hit@{} {:.3}  MRR {:.3}  nDCG@{} {:.3}",
        report.cases,
        report.mode.as_str(),
        report.k,
        report.hit_at_k,
        report.mrr,
        report.k,
        report.ndcg_at_k
    );
    Ok(())
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_tune(
    content_dir: PathBuf,
    cms: Cms,
    eval: Option<PathBuf>,
    save_eval: Option<PathBuf>,
    interactive: bool,
    model_id: &str,
    chunk_sizes: &str,
    overlaps: &str,
    top_k: usize,
    mode: SearchMode,
    report: Option<PathBuf>,
) -> Result<()> {
    let mode: Mode = mode.into();
    let parser = parser_for(cms);
    eprintln!(
        "Parsing content from {} with {} parser...",
        content_dir.display(),
        cms.as_str()
    );
    let docs = parse_content_dir(&content_dir, parser.as_ref())?;
    eprintln!("  Found {} documents", docs.len());

    let mut suite = if let Some(path) = &eval {
        eprintln!("Loading acceptance suite: {}", path.display());
        load_suite(path)?
    } else {
        AcceptanceSuite {
            name: Some("interactive-suite".to_string()),
            cases: Vec::new(),
        }
    };

    if interactive {
        interactive_collect_cases(
            &mut suite,
            &docs,
            model_id,
            chunk_sizes,
            overlaps,
            top_k,
            mode,
        )?;
        let persist_path = save_eval.or(eval);
        if let Some(path) = persist_path {
            write_suite(&path, &suite)?;
            eprintln!("Saved acceptance suite to {}", path.display());
        }
    }

    if suite.cases.is_empty() {
        bail!("no acceptance cases available. pass --eval or use --interactive to build one");
    }

    let candidates = run_tuning(&docs, &suite, model_id, chunk_sizes, overlaps, top_k, mode)?;
    if candidates.is_empty() {
        bail!("no tuning candidates were produced");
    }

    println!("\nTuning results (best first)");
    println!("{}", "-".repeat(72));
    for (i, c) in candidates.iter().enumerate() {
        println!(
            "{}. chunk_size={}, overlap={} | pass={}/{} ({:.1}%) | weighted={:.2}/{:.2}",
            i + 1,
            c.chunk_size,
            c.overlap,
            c.passed_cases,
            c.total_cases,
            c.pass_rate * 100.0,
            c.weighted_score,
            c.weighted_total
        );
    }

    if let Some(best) = candidates.first() {
        println!(
            "\nRecommended: --chunk-size {} --overlap {}",
            best.chunk_size, best.overlap
        );
    }

    if let Some(report_path) = report {
        let json = serde_json::to_string_pretty(&candidates)?;
        fs::write(&report_path, json)
            .with_context(|| format!("writing report {}", report_path.display()))?;
        eprintln!("Wrote tune report to {}", report_path.display());
    }

    Ok(())
}

fn parser_for(cms: Cms) -> Box<dyn ContentParser> {
    match cms {
        Cms::Hugo => Box::new(HugoParser),
        Cms::Astro => Box::new(AstroParser),
        Cms::Docusaurus => Box::new(DocusaurusParser),
        Cms::Mkdocs => Box::new(MkDocsParser),
        Cms::Eleventy => Box::new(EleventyParser),
        Cms::Jekyll => Box::new(JekyllParser),
    }
}

fn cmd_qa_corpus(
    index_path: PathBuf,
    output: PathBuf,
    ollama_model: Option<String>,
    ollama_url: String,
    ollama_max_chunks: usize,
    ollama_max_pairs_per_chunk: usize,
    ollama_temperature: f32,
) -> Result<()> {
    eprintln!("Loading index from {}...", index_path.display());
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;

    let mut corpus = if !index.qa.is_empty() {
        eprintln!("Using embedded QA section from index...");
        QaCorpus {
            version: 1,
            entries: index.qa.clone(),
        }
    } else {
        if index.texts.is_empty() {
            bail!(
                "index does not contain chunk texts (v2 index). rebuild index with current eddie first"
            );
        }
        let built = build_qa_corpus_from_chunks(&index.texts, &index.metadata);
        eprintln!("Heuristic QA entries: {}", built.entries.len());
        built
    };

    if let Some(model) = ollama_model {
        if index.texts.is_empty() {
            bail!("index does not contain chunk texts required for synthesis");
        }
        eprintln!("Running Ollama synthesis with model {}...", model);
        let cfg = OllamaConfig {
            model,
            endpoint: ollama_url,
            max_chunks: ollama_max_chunks,
            max_pairs_per_chunk: ollama_max_pairs_per_chunk,
            temperature: ollama_temperature,
            ..Default::default()
        };
        let llm_entries = synthesize_with_ollama_from_chunks(&index.texts, &index.metadata, &cfg)?;
        eprintln!("Ollama QA entries: {}", llm_entries.len());
        corpus.entries.extend(llm_entries);
        corpus.dedup();
    }

    let json = serde_json::to_string_pretty(&corpus)?;
    fs::write(&output, json).with_context(|| format!("writing {}", output.display()))?;

    eprintln!(
        "Done. Wrote {} QA entries to {}",
        corpus.entries.len(),
        output.display()
    );

    Ok(())
}

fn cmd_claims_corpus(
    index_path: PathBuf,
    output: PathBuf,
    claims_edits: Option<PathBuf>,
) -> Result<()> {
    eprintln!("Loading index from {}...", index_path.display());
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;

    let mut corpus = if !index.claims.is_empty() {
        eprintln!("Using embedded claims section from index...");
        eddie::claims::ClaimCorpus {
            version: 1,
            claims: index.claims.clone(),
        }
    } else {
        if index.texts.is_empty() {
            bail!("index does not contain chunk texts. rebuild index with current eddie first");
        }
        build_claim_corpus_from_chunks(&index.texts, &index.metadata)
    };
    if let Some(path) = claims_edits {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading claims edits {}", path.display()))?;
        let edits = parse_claim_edits_toml(&raw)?;
        apply_claim_edits(&mut corpus.claims, &edits);
    }
    corpus.dedup();

    let json = serde_json::to_string_pretty(&corpus)?;
    fs::write(&output, json).with_context(|| format!("writing {}", output.display()))?;
    eprintln!(
        "Done. Wrote {} claims to {}",
        corpus.claims.len(),
        output.display()
    );
    Ok(())
}

fn run_tuning(
    docs: &[Document],
    suite: &AcceptanceSuite,
    model_id: &str,
    chunk_sizes: &str,
    overlaps: &str,
    default_top_k: usize,
    mode: Mode,
) -> Result<Vec<TuneCandidate>> {
    let chunk_values = parse_usize_csv(chunk_sizes)?;
    let overlap_values = parse_usize_csv(overlaps)?;

    let embedder = if matches!(mode, Mode::Dense | Mode::Hybrid) {
        eprintln!("Loading embedding model {} for tuning...", model_id);
        let device = select_device(DevicePref::Auto)?;
        Some(load_dense(model_id, &device, &DenseOverrides::default())?)
    } else {
        None
    };
    if mode == Mode::Sparse {
        bail!("tune cannot build the sparse arm yet; use --mode hybrid, dense, or keyword");
    }

    let query_embeddings: Option<Vec<Vec<f32>>> = match &embedder {
        Some(embedder) => {
            let mut rows = Vec::with_capacity(suite.cases.len());
            for case in &suite.cases {
                rows.push(embed_query(embedder.as_ref(), &case.query)?);
            }
            Some(rows)
        }
        None => None,
    };

    let mut candidates = Vec::new();

    for &chunk_size in &chunk_values {
        for &overlap in &overlap_values {
            eprintln!(
                "Evaluating chunk_size={}, overlap={}...",
                chunk_size, overlap
            );
            let index = build_index_in_memory(docs, chunk_size, overlap, embedder.as_deref())?;

            let mut case_reports = Vec::new();
            for (case_idx, case) in suite.cases.iter().enumerate() {
                let top_k = case.top_k.unwrap_or(default_top_k);
                let query_vec = query_embeddings
                    .as_ref()
                    .map(|rows| rows[case_idx].as_slice());
                let ids = retrieve_chunk_ids(&index, &case.query, query_vec, top_k, mode)?;
                let context = build_eval_context(&index, &ids);
                case_reports.push(evaluate_case(case, &context));
            }

            let summary = summarize(case_reports, suite);
            candidates.push(TuneCandidate {
                chunk_size,
                overlap,
                passed_cases: summary.passed_cases,
                total_cases: summary.total_cases,
                pass_rate: summary.pass_rate,
                weighted_score: summary.weighted_score,
                weighted_total: summary.weighted_total,
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.weighted_score
            .total_cmp(&a.weighted_score)
            .then_with(|| b.pass_rate.total_cmp(&a.pass_rate))
            .then_with(|| a.chunk_size.cmp(&b.chunk_size))
            .then_with(|| a.overlap.cmp(&b.overlap))
    });

    Ok(candidates)
}

/// Build the index `eddie index` would ship for these parameters (int8
/// dense lane, BM25), without writing it.
fn build_index_in_memory(
    docs: &[Document],
    chunk_size: usize,
    overlap: usize,
    encoder: Option<&dyn DenseEncoder>,
) -> Result<SearchIndex> {
    // `tune` evaluates the fine lane with heading chunking, sized in the
    // encoder's tokens when one is loaded.
    let mut all_chunks = Vec::new();
    for doc in docs {
        let mut chunks = match encoder {
            Some(enc) => {
                let budget = chunk_size.min(enc.spec().max_seq_len);
                let count = |t: &str| enc.count_tokens(t);
                chunk_document_with_budget(doc, budget, overlap, ChunkStrategy::Heading, &count)
            }
            None => chunk_document_with_strategy(doc, chunk_size, overlap, ChunkStrategy::Heading),
        };
        for chunk in &mut chunks {
            chunk.meta.granularity = Some("fine".to_string());
        }
        all_chunks.extend(chunks);
    }
    let all_chunks = dedupe_chunks(all_chunks);

    let metadata: Vec<_> = all_chunks.iter().map(|c| c.meta.clone()).collect();
    let texts: Vec<String> = all_chunks.iter().map(|c| c.text.clone()).collect();
    let n = texts.len();
    let overlap_words = vec![0u16; n];

    let mut builder = IndexBuilder::new();
    if let Some(enc) = encoder {
        let inputs: Vec<String> = all_chunks.iter().map(|c| c.embed_text()).collect();
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let vectors = embed_texts_with(enc, &refs, TextKind::Document, DEFAULT_BATCH_SIZE)?;
        let dim = enc.dim();
        builder.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(enc.spec().clone(), dim, n, &vectors, Quant::Int8)?,
        )?;
    }
    builder.add_chunks(metadata, texts, overlap_words)?;
    builder.finish()
}

/// Best chunk id of each of the `top_k` pages the widget would show.
fn retrieve_chunk_ids(
    index: &SearchIndex,
    query: &str,
    query_vec: Option<&[f32]>,
    top_k: usize,
    mode: Mode,
) -> Result<Vec<usize>> {
    let dense = match (mode, query_vec) {
        (Mode::Dense | Mode::Hybrid, Some(v)) => Some((0usize, v.to_vec())),
        (Mode::Dense | Mode::Hybrid, None) => {
            bail!("{} mode requires a query embedding", mode.as_str())
        }
        _ => None,
    };
    let q = Query {
        text: query,
        dense,
        sparse: None,
        mode,
        top_k,
        weights: Weights::default(),
    };
    let retrieval = retrieve(index, &q)?;
    let pages = group_pages(index, &retrieval.ranked, &query_terms(query), top_k);
    Ok(pages.into_iter().map(|p| p.chunk).collect())
}

fn build_eval_context(index: &SearchIndex, ids: &[usize]) -> String {
    let mut out = String::new();
    for id in ids {
        if let (Some(meta), Some(text)) = (index.metadata.get(*id), index.texts.get(*id)) {
            out.push_str(&meta.title);
            out.push('\n');
            out.push_str(&meta.url);
            out.push('\n');
            if let Some(section) = &meta.section {
                out.push_str(section);
                out.push('\n');
            }
            out.push_str(text);
            out.push_str("\n\n");
        }
    }
    out
}

/// Embed `texts` as one flat `len × dim` vector, printing progress. Each call
/// into the encoder covers several batches so it can sort by length and pad
/// less.
fn embed_texts_with(
    encoder: &dyn DenseEncoder,
    texts: &[&str],
    kind: TextKind,
    batch_size: usize,
) -> Result<Vec<f32>> {
    let dim = encoder.dim();
    let mut out = Vec::with_capacity(texts.len() * dim);
    let group = batch_size.max(1) * 8;
    let mut done = 0usize;
    for chunk in texts.chunks(group) {
        let vecs = encoder.embed(chunk, kind)?;
        for vec in vecs {
            debug_assert_eq!(vec.len(), dim);
            out.extend(vec);
        }
        done += chunk.len();
        if done.is_multiple_of(group * 4) || done == texts.len() {
            eprintln!("  Embedded {}/{} texts", done, texts.len());
        }
    }
    Ok(out)
}

fn report_lane_timing(lane: &dyn DenseEncoder, count: usize, secs: f64) {
    let spec = lane.spec();
    eprintln!(
        "  lane '{}': {} texts in {:.1}s ({:.1} texts/s), {} truncated at {} tokens",
        spec.id,
        count,
        secs,
        count as f64 / secs.max(1e-9),
        lane.truncated_count(),
        spec.max_seq_len
    );
}

/// Reconcile `--preset` with explicit flags. Explicit `--dense-model`,
/// `--sparse`/`--sparse-model` and a non-`auto` `--device` win over the preset.
#[allow(clippy::too_many_arguments)]
fn resolve_index_models(
    preset: Option<Preset>,
    dense_model: Vec<String>,
    model_alias: Option<String>,
    dense_runtime: &[String],
    sparse: bool,
    sparse_model: Option<String>,
    device: &str,
    batch_size: usize,
) -> Result<IndexModelOptions> {
    let mut dense: Vec<String> = dense_model;
    if let Some(m) = model_alias {
        eprintln!("warning: --model is deprecated; use --dense-model");
        dense.push(m);
    }
    let mut sparse_model =
        sparse_model.or_else(|| sparse.then(|| models::DEFAULT_SPARSE_MODEL.to_string()));
    let mut device_pref: DevicePref = device.parse()?;

    let (preset_dense, preset_sparse, preset_cuda): (&[&str], bool, bool) = match preset {
        None | Some(Preset::Fast) => (&[models::DEFAULT_DENSE_MODEL], false, false),
        Some(Preset::Balanced) => (&["BAAI/bge-small-en-v1.5"], true, false),
        Some(Preset::Quality) => (
            &["BAAI/bge-small-en-v1.5", "Qwen/Qwen3-Embedding-0.6B"],
            true,
            false,
        ),
        Some(Preset::Gpu) => (
            &["BAAI/bge-small-en-v1.5", "Qwen/Qwen3-Embedding-0.6B"],
            true,
            true,
        ),
    };
    if dense.is_empty() {
        dense = preset_dense.iter().map(|s| s.to_string()).collect();
    }
    if sparse_model.is_none() && preset_sparse {
        sparse_model = Some(models::DEFAULT_SPARSE_MODEL.to_string());
    }
    if preset_cuda && device_pref == DevicePref::Auto {
        device_pref = DevicePref::Cuda(0);
    }

    let dense_models: Vec<String> = dense
        .into_iter()
        .map(|name| {
            models::lookup_lane_or_model(&name)
                .map(|m| m.model.to_string())
                .unwrap_or(name)
        })
        .collect();
    if dense_runtime.len() > dense_models.len() {
        bail!(
            "{} --dense-runtime values for {} dense lanes",
            dense_runtime.len(),
            dense_models.len()
        );
    }
    let mut dense_runtimes = Vec::with_capacity(dense_models.len());
    for i in 0..dense_models.len() {
        dense_runtimes.push(match dense_runtime.get(i) {
            Some(spec) => parse_runtime_spec(spec)?,
            None => None,
        });
    }
    if batch_size == 0 {
        bail!("--batch-size must be at least 1");
    }
    Ok(IndexModelOptions {
        dense_models,
        dense_runtimes,
        sparse_model,
        device: device_pref,
        batch_size,
    })
}

/// `wasm-candle`, `webgpu-onnx:<repo>:<dtype>[:<dtype_f16>]`, or `auto`/`` for
/// the registry default.
fn parse_runtime_spec(spec: &str) -> Result<Option<RuntimeSpec>> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if spec.eq_ignore_ascii_case("wasm-candle") {
        return Ok(Some(models::runtime_spec(models::RuntimeKind::WasmCandle)));
    }
    let Some(rest) = spec.strip_prefix("webgpu-onnx:") else {
        bail!(
            "unknown --dense-runtime '{spec}' (expected wasm-candle or webgpu-onnx:<repo>:<dtype>[:<dtype_f16>])"
        );
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("--dense-runtime webgpu-onnx needs <repo>:<dtype>[:<dtype_f16>], got '{spec}'");
    }
    Ok(Some(RuntimeSpec::WebgpuOnnx {
        repo: parts[0].to_string(),
        dtype: parts[1].to_string(),
        dtype_f16: parts
            .get(2)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        // The runtime pools with the lane's own pooling; last_token for
        // decoder embedders is the common case for ONNX ports.
        pooling: "last_token".to_string(),
    }))
}

fn parse_usize_csv(csv: &str) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    for part in csv.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = trimmed
            .parse::<usize>()
            .with_context(|| format!("parsing '{}' as usize", trimmed))?;
        if value == 0 {
            bail!("values must be > 0");
        }
        out.push(value);
    }

    if out.is_empty() {
        bail!("expected at least one numeric value in '{}'", csv);
    }

    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn interactive_collect_cases(
    suite: &mut AcceptanceSuite,
    docs: &[Document],
    model_id: &str,
    chunk_sizes: &str,
    overlaps: &str,
    top_k: usize,
    mode: Mode,
) -> Result<()> {
    eprintln!("Interactive tuning: press Enter on query to finish.");

    loop {
        let query = prompt("Query")?;
        if query.trim().is_empty() {
            break;
        }

        let must_any_raw = prompt("Expected phrases (use | separator, at least one)")?;
        let must_match_any: Vec<String> = must_any_raw
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        if must_match_any.is_empty() {
            eprintln!("Skipping case: no expected phrase provided.");
            continue;
        }

        let must_all_raw = prompt("Required phrases (optional, use | separator)")?;
        let must_include_all: Vec<String> = must_all_raw
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();

        let rating_raw = prompt("Rate current relevance 1-5 (optional)")?;
        let user_rating = rating_raw
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|v| (1..=5).contains(v));

        let id = format!("interactive-{}", suite.cases.len() + 1);
        suite.cases.push(AcceptanceCase {
            id,
            query: query.trim().to_string(),
            must_match_any,
            must_include_all,
            top_k: Some(top_k),
            weight: rating_weight(user_rating),
            user_rating,
        });

        let candidates = run_tuning(docs, suite, model_id, chunk_sizes, overlaps, top_k, mode)?;
        if let Some(best) = candidates.first() {
            eprintln!(
                "Best so far: chunk_size={}, overlap={} | pass={}/{} | weighted={:.2}/{:.2}",
                best.chunk_size,
                best.overlap,
                best.passed_cases,
                best.total_cases,
                best.weighted_score,
                best.weighted_total
            );
        }
    }

    suite.validate()?;
    Ok(())
}

fn rating_weight(rating: Option<u8>) -> f32 {
    match rating {
        Some(1) => 2.0,
        Some(2) => 1.75,
        Some(3) => 1.5,
        Some(4) => 1.25,
        Some(5) => 1.0,
        _ => 1.0,
    }
}

fn prompt(label: &str) -> Result<String> {
    print!("{}: ", label);
    io::stdout().flush().context("flushing stdout")?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("reading interactive input")?;
    Ok(buf.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usize_csv_dedupes_and_sorts() {
        let out = parse_usize_csv("256, 128,256").unwrap();
        assert_eq!(out, vec![128, 256]);
    }

    #[test]
    fn parse_usize_csv_rejects_empty() {
        assert!(parse_usize_csv(" , ").is_err());
    }

    #[test]
    fn presets_and_flags_reconcile() {
        let opts = resolve_index_models(None, vec![], None, &[], false, None, "auto", 32).unwrap();
        assert_eq!(
            opts.dense_models,
            vec![models::DEFAULT_DENSE_MODEL.to_string()]
        );
        assert_eq!(opts.sparse_model, None);
        assert_eq!(opts.device, DevicePref::Auto);

        let opts = resolve_index_models(
            Some(Preset::Gpu),
            vec![],
            None,
            &[],
            false,
            None,
            "auto",
            16,
        )
        .unwrap();
        assert_eq!(opts.dense_models.len(), 2);
        assert_eq!(
            opts.sparse_model.as_deref(),
            Some(models::DEFAULT_SPARSE_MODEL)
        );
        assert_eq!(opts.device, DevicePref::Cuda(0));

        // Explicit flags win; lane ids resolve to model ids.
        let opts = resolve_index_models(
            Some(Preset::Quality),
            vec!["qwen3e".into()],
            None,
            &["webgpu-onnx:org/repo:q4:q4f16".into()],
            false,
            None,
            "cpu",
            8,
        )
        .unwrap();
        assert_eq!(
            opts.dense_models,
            vec!["Qwen/Qwen3-Embedding-0.6B".to_string()]
        );
        assert_eq!(opts.device, DevicePref::Cpu);
        assert!(matches!(
            &opts.dense_runtimes[0],
            Some(RuntimeSpec::WebgpuOnnx { repo, dtype_f16: Some(f16), .. })
                if repo == "org/repo" && f16 == "q4f16"
        ));
        assert!(
            resolve_index_models(
                None,
                vec![],
                None,
                &["x".into(), "y".into()],
                false,
                None,
                "cpu",
                8
            )
            .is_err()
        );
        assert!(parse_runtime_spec("tpu").is_err());
        assert!(parse_runtime_spec("auto").unwrap().is_none());
    }

    #[test]
    fn rating_weight_map() {
        assert_eq!(rating_weight(Some(1)), 2.0);
        assert_eq!(rating_weight(Some(5)), 1.0);
        assert_eq!(rating_weight(None), 1.0);
    }
}
