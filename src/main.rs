// SPDX-License-Identifier: GPL-3.0-only

//! Eddie CLI: build-time indexer for static site content.

use std::collections::{BTreeSet, HashMap, HashSet};
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
    ndcg_graded_at_k, summarize, write_suite,
};
use eddie::index::{
    DenseLane, IndexBuilder, SCOPE_CHUNKS, SCOPE_CLAIMS, SCOPE_QA, SearchIndex, context_prefix,
    with_context,
};
use eddie::manifest::{DenseSpec, Quant, RuntimeSpec, SparseSpec, SparseTerm};
use eddie::models;
use eddie::parse::{
    AstroParser, ContentParser, DocusaurusParser, EleventyParser, HtmlOptions, HtmlParser,
    HugoParser, JekyllParser, MkDocsParser, parse_content_dir, parse_content_dir_report,
};
use eddie::qa::{
    OllamaConfig, OpenRouterConfig, QaCorpus, QaEntry, build_qa_corpus_from_chunks_with_subject,
    build_qa_entries_from_chunks_with_subject, synthesize_with_ollama_from_chunks,
    synthesize_with_openrouter_from_chunks,
};
use eddie::search::{
    Mode, PageResult, QaHit, Query, Retrieval, Weights, group_pages, qa_fetch_k, query_terms,
    rank_qa, retrieve,
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

        /// With --cms html, also index pages whose <meta name="robots"> says noindex.
        #[arg(long, default_value_t = false)]
        include_noindex: bool,

        /// Fusion weights to bake into the index as dense,sparse,bm25 (e.g. the best row of `eddie eval --sweep`); default 1,1.2,1.
        #[arg(long, value_name = "D,S,B")]
        weights: Option<String>,

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
        /// `wasm-candle` (BERT family only) or
        /// `webgpu-onnx:<repo>:<dtype>[:<dtype_f16>[:<pooling>]]`; pooling
        /// defaults to the lane's own (mean, cls or last_token).
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

        /// Kept for compatibility: the summary chunk (title, description,
        /// headings) is on by default since 0.4.1; see --no-summary-lane.
        #[arg(long, default_value_t = false, hide = true)]
        summary_lane: bool,

        /// Skip the per-page summary chunk (title + description + headings).
        #[arg(long, default_value_t = false)]
        no_summary_lane: bool,

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

        /// Name the site's owner in QA entries (e.g. "Jason Grey"): the LLM
        /// prompts ask for it, generated "the author"/"the subject" is
        /// rewritten to it, and the heuristics use it.
        #[arg(long = "qa-subject", alias = "subject", value_name = "NAME")]
        qa_subject: Option<String>,

        /// Index chunk text as stored, without the "{title} — {section}"
        /// line that is otherwise prepended to what the dense, sparse and
        /// BM25 arms see (stored texts stay clean either way).
        #[arg(long, default_value_t = false)]
        no_title_context: bool,
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

        /// Also print, per result, the per-arm ranks of its best chunk and
        /// the title/section prefix the index text carried.
        #[arg(long, default_value_t = false)]
        explain: bool,

        /// RRF weights as dense,sparse,bm25 (default 1,1.2,1).
        #[arg(long, value_name = "D,S,B")]
        weights: Option<String>,

        /// Candidates fetched per arm before fusion (default max(3·top_k, 30)).
        #[arg(long)]
        fetch_k: Option<usize>,

        /// RRF constant k (default 60).
        #[arg(long)]
        rrf_k: Option<f64>,
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

        /// RRF weights as dense,sparse,bm25 (default 1,1.2,1).
        #[arg(long, value_name = "D,S,B")]
        weights: Option<String>,

        /// Candidates fetched per arm before fusion (default max(3·top_k, 30)).
        #[arg(long)]
        fetch_k: Option<usize>,

        /// RRF constant k (default 60).
        #[arg(long)]
        rrf_k: Option<f64>,

        /// Evaluate a weight grid (dense 0.8/1/1.2 × sparse 0.6..1.2 × bm25
        /// 0.6..1.2) and print it sorted by nDCG, then MRR.
        #[arg(long, default_value_t = false)]
        sweep: bool,

        /// Use graded nDCG from each case's `[cases.graded]` table
        /// (url = grade 1..3); cases without one score their `relevant`
        /// urls at grade 1.
        #[arg(long, default_value_t = false)]
        graded: bool,

        /// Also report each arm on its own (dense, sparse, keyword) next to hybrid.
        #[arg(long, default_value_t = false)]
        all_modes: bool,
    },

    /// Rank the QA entries of an index for a query and show the score components.
    Qa {
        /// Path to the index file.
        #[arg(long)]
        index: PathBuf,

        /// Query text.
        #[arg(long)]
        query: String,

        /// Number of QA hits to print.
        #[arg(long, default_value = "5")]
        k: usize,

        /// Dense lane id to embed the query with (default: the index's first lane).
        #[arg(long)]
        lane: Option<String>,

        /// Print the hits as JSON.
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

        /// Seed passed to the Ollama model for reproducible synthesis.
        #[arg(long)]
        qa_seed: Option<u64>,

        /// Also run the regex heuristics that guess QA pairs from prose (off by default; tuned for resume-style pages).
        #[arg(long, default_value_t = false)]
        qa_heuristics: bool,

        /// Name the site's owner in QA entries (see `eddie index --qa-subject`).
        #[arg(long = "qa-subject", alias = "subject", value_name = "NAME")]
        qa_subject: Option<String>,
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

        /// Also run the regex heuristics that extract claims from prose (off by default).
        #[arg(long, default_value_t = false)]
        claims_heuristics: bool,
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
    /// Rendered HTML output (e.g. a Hugo `public/` build directory), for
    /// sites whose copy lives in templates rather than markdown content
    /// files. Point `--content-dir` at the built output, not the source.
    Html,
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
            Cms::Html => "html",
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
            include_noindex,
            weights,
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
            no_summary_lane,
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
            qa_subject,
            no_title_context,
        } => cmd_index(
            content_dir,
            cms,
            include_noindex,
            weights.as_deref(),
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
            summary_lane || !no_summary_lane,
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
            qa_subject,
            !no_title_context,
        ),
        Command::Search {
            index,
            query,
            top_k,
            mode,
            lane,
            json,
            explain,
            weights,
            fetch_k,
            rrf_k,
        } => cmd_search(
            index,
            &query,
            top_k,
            mode.into(),
            lane.as_deref(),
            json,
            explain,
            ranking_options(weights.as_deref(), fetch_k, rrf_k)?,
        ),
        Command::Stats { index, json } => cmd_stats(index, json),
        Command::Eval {
            index,
            labels,
            top_k,
            mode,
            lane,
            json,
            weights,
            fetch_k,
            rrf_k,
            sweep,
            graded,
            all_modes,
        } => cmd_eval(
            index,
            labels,
            top_k,
            mode.into(),
            lane.as_deref(),
            json,
            EvalOptions {
                ranking: ranking_options(weights.as_deref(), fetch_k, rrf_k)?,
                sweep,
                graded,
                all_modes,
            },
        ),
        Command::Qa {
            index,
            query,
            k,
            lane,
            json,
        } => cmd_qa(index, &query, k, lane.as_deref(), json),
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
            qa_seed,
            qa_heuristics,
            qa_subject,
        } => cmd_qa_corpus(
            index,
            output,
            ollama_model,
            ollama_url,
            ollama_max_chunks,
            ollama_max_pairs_per_chunk,
            ollama_temperature,
            qa_seed,
            qa_heuristics,
            qa_subject,
        ),
        Command::ClaimsCorpus {
            index,
            output,
            claims_edits,
            claims_heuristics,
        } => cmd_claims_corpus(index, output, claims_edits, claims_heuristics),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_index(
    content_dir: PathBuf,
    cms: Cms,
    include_noindex: bool,
    fusion_weights: Option<&str>,
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
    qa_subject: Option<String>,
    title_context: bool,
) -> Result<()> {
    // Sub-flags only take effect with their section flag; say so up front
    // rather than silently building an index without the section.
    if !qa_enabled {
        let given = flags_given(&[
            (qa_heuristics, "--qa-heuristics"),
            (qa_seed.is_some(), "--qa-seed"),
            (qa_ollama_model.is_some(), "--qa-ollama-model"),
            (qa_openrouter_model.is_some(), "--qa-openrouter-model"),
            (qa_subject.is_some(), "--qa-subject"),
        ]);
        if !given.is_empty() {
            eprintln!(
                "warning: {} ignored without --qa; no qa section will be built",
                given.join(", ")
            );
        }
    }
    if !claims_enabled {
        let given = flags_given(&[
            (claims_heuristics, "--claims-heuristics"),
            (claims_edits_path.is_some(), "--claims-edits"),
        ]);
        if !given.is_empty() {
            eprintln!(
                "warning: {} ignored without --claims; no claims section will be built",
                given.join(", ")
            );
        }
    }

    // Parse content
    eprintln!(
        "Parsing content from {} with {} parser...",
        content_dir.display(),
        cms.as_str()
    );
    let parser = parser_for_with(cms, include_noindex);
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
        warn_about_lane_runtime(spec);
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
    let prefix_tokens = doc_prefix_tokens(primary);
    let budget = chunk_budget(primary, chunk_size)?;
    let count = |text: &str| primary.count_tokens(text);
    eprintln!(
        "Chunking documents (strategy: {:?}, budget {} tokens{}, overlap {} tokens, counted with lane '{}')...",
        chunk_strategy,
        budget,
        if prefix_tokens > 0 {
            format!(
                " after {} for the doc prefix {:?}",
                prefix_tokens,
                primary.spec().doc_prefix
            )
        } else {
            String::new()
        },
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
            let coarse_budget = chunk_budget(primary, coarse_size)?;
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

    // What each arm sees. With title context (default) every indexed text
    // starts with a "{title} — {section}" line, so a query that names the
    // page matches a chunk whose body never repeats the title. Stored texts
    // (display, snippets) stay clean.
    //   dense:  prefix + "\n" + overlap prefix + text
    //   sparse: prefix + "\n" + text
    //   bm25:   prefix + "\n" + text
    let prefixes: Vec<String> = all_chunks
        .iter()
        .map(|c| {
            if title_context {
                context_prefix(&c.meta)
            } else {
                String::new()
            }
        })
        .collect();
    eprintln!(
        "Title context: {}",
        if title_context {
            "on (\"{title} — {section}\" prefixed to the indexed text; --no-title-context disables)"
        } else {
            "off"
        }
    );
    let embed_inputs: Vec<String> = all_chunks
        .iter()
        .zip(&prefixes)
        .map(|(c, p)| with_context(p, &c.embed_text()))
        .collect();
    let embed_refs: Vec<&str> = embed_inputs.iter().map(String::as_str).collect();
    let index_texts: Vec<String> = all_chunks
        .iter()
        .zip(&prefixes)
        .map(|(c, p)| with_context(p, &c.text))
        .collect();
    let texts: Vec<&str> = index_texts.iter().map(String::as_str).collect();
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
        if let Some(subject) = &qa_subject {
            eprintln!("  QA subject: {}", subject);
        }
        if qa_heuristics {
            qa_entries = build_qa_entries_from_chunks_with_subject(
                &fact_texts,
                &fact_metadata,
                qa_subject.as_deref().unwrap_or(eddie::qa::DEFAULT_SUBJECT),
            );
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
                subject: qa_subject.clone(),
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
                subject: qa_subject.clone(),
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
        if !claims_heuristics && claims_edits_path.is_none() {
            eprintln!(
                "  warning: --claims without --claims-heuristics and without --claims-edits produces no entries"
            );
        }
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
        corpus.dedup();
        claims = corpus.claims;
        eprintln!("  Claims entries: {}", claims.len());
    }

    // --- Index assembly (format v5) -------------------------------------
    let n = metadata.len();
    // Stored texts are the clean chunk text (no overlap prefix), so the
    // builder has nothing to strip.
    let overlap_words: Vec<u16> = vec![0; n];

    let mut builder = IndexBuilder::new();
    builder.add_chunks_indexed(metadata, chunk_texts, index_texts, overlap_words)?;
    builder.title_context(title_context);
    if let Some(spec) = fusion_weights {
        let w = Weights::parse(spec)?;
        builder.fusion(Some(eddie::manifest::FusionWeights {
            dense: w.dense,
            sparse: w.sparse,
            bm25: w.bm25,
        }));
    }
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
            lane.reset_truncated_count();
            let started = Instant::now();
            let vectors = embed_texts_with(
                lane.as_ref(),
                &refs,
                TextKind::Document,
                model_opts.batch_size,
            )?;
            report_lane_timing(lane.as_ref(), refs.len(), started.elapsed().as_secs_f64());
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
            lane.reset_truncated_count();
            let started = Instant::now();
            let vectors = embed_texts_with(
                lane.as_ref(),
                &refs,
                TextKind::Document,
                model_opts.batch_size,
            )?;
            report_lane_timing(lane.as_ref(), refs.len(), started.elapsed().as_secs_f64());
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
            family: Some(spec.family),
            pooling: Some(spec.pooling),
            max_seq_len: Some(spec.max_seq_len),
            query_prefix: Some(spec.query_prefix.clone()),
            doc_prefix: Some(spec.doc_prefix.clone()),
            normalize: Some(spec.normalize),
            runtime: Some(spec.runtime.clone()),
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

fn runtime_kind(runtime: &RuntimeSpec) -> &'static str {
    match runtime {
        RuntimeSpec::WasmCandle { .. } => "wasm-candle",
        RuntimeSpec::WebgpuOnnx { .. } => "webgpu-onnx",
    }
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

/// Fusion knobs the CLI can override (`--weights`, `--fetch-k`, `--rrf-k`);
/// `None` keeps the defaults the widget uses.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct RankingOptions {
    weights: Option<Weights>,
    fetch_k: Option<usize>,
    rrf_k: Option<f64>,
}

fn ranking_options(
    weights: Option<&str>,
    fetch_k: Option<usize>,
    rrf_k: Option<f64>,
) -> Result<RankingOptions> {
    let weights = weights.map(Weights::parse).transpose()?;
    if fetch_k == Some(0) {
        bail!("--fetch-k must be > 0");
    }
    if let Some(k) = rrf_k
        && !(k.is_finite() && k >= 0.0)
    {
        bail!("--rrf-k must be a finite number >= 0");
    }
    Ok(RankingOptions {
        weights,
        fetch_k,
        rrf_k,
    })
}

/// The per-query arm inputs, embedded once so several fusion settings can
/// be scored against the same vectors.
struct QueryInputs {
    dense: Option<(usize, Vec<f32>)>,
    sparse: Option<Vec<SparseTerm>>,
}

/// Everything the CLI needs to run queries against an index the way the
/// widget does.
struct QueryRuntime {
    dense: Option<QueryEmbedder>,
    sparse_tokenizer: Option<tokenizers::Tokenizer>,
    mode: Mode,
    options: RankingOptions,
}

impl QueryRuntime {
    fn with_options(
        index: &SearchIndex,
        mode: Mode,
        lane: Option<&str>,
        options: RankingOptions,
    ) -> Result<Self> {
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
                None
            };
        Ok(Self {
            dense,
            sparse_tokenizer,
            mode,
            options,
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

    /// Embed the query for every arm this runtime loaded.
    fn inputs(&self, index: &SearchIndex, text: &str) -> Result<QueryInputs> {
        let dense = match &self.dense {
            Some(e) => Some((e.lane, e.embed(text)?)),
            None => None,
        };
        Ok(QueryInputs {
            dense,
            sparse: self.sparse_terms(index, text)?,
        })
    }

    /// Retrieve and group pages exactly like the widget.
    fn run(
        &self,
        index: &SearchIndex,
        text: &str,
        top_k: usize,
    ) -> Result<(Vec<PageResult>, Retrieval)> {
        let inputs = self.inputs(index, text)?;
        run_query(index, &inputs, text, top_k, self.mode, self.options)
    }
}

/// `retrieve` + `group_pages` for prepared inputs under one fusion setting.
fn run_query(
    index: &SearchIndex,
    inputs: &QueryInputs,
    text: &str,
    top_k: usize,
    mode: Mode,
    options: RankingOptions,
) -> Result<(Vec<PageResult>, Retrieval)> {
    let q = Query {
        text,
        dense: inputs.dense.clone(),
        sparse: inputs.sparse.clone(),
        mode,
        top_k,
        weights: options.weights.unwrap_or_else(|| Weights::for_index(index)),
        fetch_k: options.fetch_k,
        rrf_k: options.rrf_k,
    };
    let retrieval = retrieve(index, &q)?;
    let pages = group_pages(index, &retrieval.ranked, &query_terms(text), top_k);
    Ok((pages, retrieval))
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
    /// Only with `--explain`: per result, the best chunk's arm ranks and
    /// the title/section prefix its indexed text carried.
    #[serde(skip_serializing_if = "Option::is_none")]
    explain: Option<Vec<ExplainRow>>,
}

#[derive(serde::Serialize)]
struct ExplainRow {
    url: String,
    chunk: usize,
    dense_rank: Option<usize>,
    sparse_rank: Option<usize>,
    bm25_rank: Option<usize>,
    /// `None` when the index was built with `--no-title-context`.
    index_prefix: Option<String>,
}

fn explain_rows(
    index: &SearchIndex,
    pages: &[PageResult],
    retrieval: &Retrieval,
) -> Vec<ExplainRow> {
    pages
        .iter()
        .map(|page| {
            let ranks = retrieval.ranked.iter().find(|c| c.chunk == page.chunk);
            ExplainRow {
                url: page.url.clone(),
                chunk: page.chunk,
                dense_rank: ranks.and_then(|c| c.dense_rank),
                sparse_rank: ranks.and_then(|c| c.sparse_rank),
                bm25_rank: ranks.and_then(|c| c.bm25_rank),
                index_prefix: index
                    .manifest
                    .title_context
                    .then(|| context_prefix(&index.metadata[page.chunk])),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn cmd_search(
    index_path: PathBuf,
    query: &str,
    top_k: usize,
    mode: Mode,
    lane: Option<&str>,
    json: bool,
    explain: bool,
    options: RankingOptions,
) -> Result<()> {
    if top_k == 0 {
        bail!("--top-k must be > 0");
    }
    let index = load_index(&index_path)?;
    let runtime = QueryRuntime::with_options(&index, mode, lane, options)?;
    let (pages, retrieval) = runtime.run(&index, query, top_k)?;

    if json {
        let out = SearchOutput {
            query,
            mode,
            dense_lane: runtime.dense.as_ref().map(|d| d.spec.id.as_str()),
            arms: retrieval.arms,
            degraded: &retrieval.degraded,
            results: &pages,
            explain: explain.then(|| explain_rows(&index, &pages, &retrieval)),
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
    if explain {
        let w = options.weights.unwrap_or_default();
        println!(
            "  fusion: weights dense={} sparse={} bm25={}, fetch_k={}, rrf_k={}; title context {}",
            w.dense,
            w.sparse,
            w.bm25,
            options
                .fetch_k
                .unwrap_or_else(|| eddie::search::fetch_k(top_k)),
            options.rrf_k.unwrap_or(eddie::search::RRF_K),
            if index.manifest.title_context {
                "on"
            } else {
                "off (chunks indexed as stored)"
            }
        );
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
        if explain && index.manifest.title_context {
            println!(
                "   index prefix: {:?}",
                context_prefix(&index.metadata[page.chunk])
            );
        }
        println!("   {}", page.snippet);
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct QaOutput<'a> {
    id: usize,
    score: f64,
    dense: f64,
    overlap: f64,
    bm25_rank: Option<usize>,
    confident: bool,
    question: &'a str,
    answer: &'a str,
    source_title: &'a str,
    source_url: &'a str,
    source_section: Option<&'a str>,
}

/// `eddie qa`: rank the QA section for a query with [`rank_qa`] and show
/// every score component, the way the widget's FAQ card sees them.
fn cmd_qa(
    index_path: PathBuf,
    query: &str,
    k: usize,
    lane: Option<&str>,
    json: bool,
) -> Result<()> {
    if k == 0 {
        bail!("--k must be > 0");
    }
    let index = load_index(&index_path)?;
    if index.qa.is_empty() {
        bail!("index has no qa section (build it with `eddie index --qa ...`)");
    }
    let mut notes = Vec::new();
    let dense_hits: Vec<(usize, f32)> = if index.manifest.dense.is_empty() {
        notes.push("index has no dense lane; ranking is lexical only".to_string());
        Vec::new()
    } else {
        let embedder = QueryEmbedder::for_index(&index, lane)?;
        match index.qa_lane(&embedder.spec.id) {
            Some(qa_lane) => qa_lane.top_k(&embedder.embed(query)?, qa_fetch_k(k))?,
            None => {
                notes.push(format!(
                    "qa section has no dense/qa/{} lane; ranking is lexical only",
                    embedder.spec.id
                ));
                Vec::new()
            }
        }
    };
    let hits: Vec<QaHit> = rank_qa(&index, query, &dense_hits, k);

    if json {
        let out: Vec<QaOutput> = hits
            .iter()
            .map(|h| {
                let e = &index.qa[h.id];
                QaOutput {
                    id: h.id,
                    score: h.score,
                    dense: h.dense,
                    overlap: h.overlap,
                    bm25_rank: h.bm25_rank,
                    confident: h.confident,
                    question: &e.question,
                    answer: &e.answer,
                    source_title: &e.source_title,
                    source_url: &e.source_url,
                    source_section: e.source_section.as_deref(),
                }
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!(
        "\nQA hits for: \"{}\"  ({} entries; score = 0.6·dense + 0.4·(0.5·overlap + 0.5·bm25), confident = score ≥ {} and (overlap ≥ {} or dense ≥ {}))",
        query,
        index.qa.len(),
        eddie::search::QA_CONFIDENT_SCORE,
        eddie::search::QA_CONFIDENT_OVERLAP,
        eddie::search::QA_CONFIDENT_DENSE
    );
    for note in &notes {
        println!("  note: {}", note);
    }
    println!(
        "  terms: {}",
        eddie::search::qa_overlap_terms(query).join(" ")
    );
    println!(
        "{:<3} {:>6} {:>6} {:>7} {:>5} {:<5} question / answer",
        "#", "score", "dense", "overlap", "bm25", "conf"
    );
    println!("{}", "-".repeat(72));
    if hits.is_empty() {
        println!("(no hits)");
    }
    for (i, h) in hits.iter().enumerate() {
        let e = &index.qa[h.id];
        println!(
            "{:<3} {:>6.3} {:>6.3} {:>7.2} {:>5} {:<5} {}",
            i + 1,
            h.score,
            h.dense,
            h.overlap,
            h.bm25_rank.map_or("-".to_string(), |r| r.to_string()),
            if h.confident { "yes" } else { "no" },
            e.question
        );
        println!("{:<37} → {}", "", e.answer);
        println!("{:<37}   [{}] {}", "", h.id, e.source_url);
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
    /// Relevant page URLs (binary relevance).
    #[serde(default)]
    relevant: Vec<String>,
    /// Optional `[cases.graded]` table: url = grade 1..3. Its urls count as
    /// relevant; `--graded` scores nDCG with the grades.
    #[serde(default)]
    graded: std::collections::BTreeMap<String, u8>,
}

impl LabelCase {
    /// Every labelled url (canonical form) with its grade: `graded` entries
    /// as given, plain `relevant` urls at grade 1.
    fn grades(&self) -> Vec<(String, u8)> {
        let mut out: Vec<(String, u8)> = self
            .graded
            .iter()
            .map(|(u, g)| (normalize_eval_url(u), *g))
            .collect();
        for url in &self.relevant {
            let u = normalize_eval_url(url);
            if !out.iter().any(|(x, _)| *x == u) {
                out.push((u, 1));
            }
        }
        out
    }
}

fn validate_label_cases(cases: &[LabelCase]) -> Result<()> {
    if cases.is_empty() {
        bail!("labels file has no [[cases]]");
    }
    for (i, c) in cases.iter().enumerate() {
        if c.query.trim().is_empty() || (c.relevant.is_empty() && c.graded.is_empty()) {
            bail!(
                "case {} needs a query and at least one relevant url (relevant = [...] or [cases.graded])",
                i + 1
            );
        }
        for (url, grade) in &c.graded {
            if !(1..=3).contains(grade) {
                bail!(
                    "case {}: graded url {:?} has grade {} (expected 1..3)",
                    i + 1,
                    url,
                    grade
                );
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
struct CaseMetrics {
    id: String,
    query: String,
    hit: f64,
    rr: f64,
    ndcg: f64,
    first_relevant_rank: Option<usize>,
    top: Vec<String>,
}

/// Mean Hit@k / MRR / nDCG@k over a case list.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
struct Means {
    hit_at_k: f64,
    mrr: f64,
    ndcg_at_k: f64,
}

fn means(per_case: &[CaseMetrics]) -> Means {
    let n = per_case.len().max(1) as f64;
    Means {
        hit_at_k: per_case.iter().map(|c| c.hit).sum::<f64>() / n,
        mrr: per_case.iter().map(|c| c.rr).sum::<f64>() / n,
        ndcg_at_k: per_case.iter().map(|c| c.ndcg).sum::<f64>() / n,
    }
}

#[derive(Debug, serde::Serialize)]
struct SweepRow {
    weights: Weights,
    #[serde(flatten)]
    means: Means,
}

#[derive(Debug, serde::Serialize)]
struct ModeRow {
    mode: Mode,
    #[serde(flatten)]
    means: Means,
}

#[derive(Debug, serde::Serialize)]
struct EvalReport {
    k: usize,
    mode: Mode,
    cases: usize,
    hit_at_k: f64,
    mrr: f64,
    ndcg_at_k: f64,
    /// `true` when nDCG used `[cases.graded]` grades (`--graded`).
    graded: bool,
    weights: Weights,
    fetch_k: usize,
    rrf_k: f64,
    per_case: Vec<CaseMetrics>,
    /// Only with `--all-modes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    modes: Option<Vec<ModeRow>>,
    /// Only with `--sweep`, best first.
    #[serde(skip_serializing_if = "Option::is_none")]
    sweep: Option<Vec<SweepRow>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct EvalOptions {
    ranking: RankingOptions,
    sweep: bool,
    graded: bool,
    all_modes: bool,
}

/// The weight grid `--sweep` evaluates.
const SWEEP_DENSE: [f64; 3] = [0.8, 1.0, 1.2];
const SWEEP_SPARSE: [f64; 4] = [0.6, 0.8, 1.0, 1.2];
const SWEEP_BM25: [f64; 4] = [0.6, 0.8, 1.0, 1.2];

fn sweep_grid() -> Vec<Weights> {
    let mut out = Vec::with_capacity(SWEEP_DENSE.len() * SWEEP_SPARSE.len() * SWEEP_BM25.len());
    for &dense in &SWEEP_DENSE {
        for &sparse in &SWEEP_SPARSE {
            for &bm25 in &SWEEP_BM25 {
                out.push(Weights {
                    dense,
                    sparse,
                    bm25,
                });
            }
        }
    }
    out
}

/// Sort sweep rows by nDCG, then MRR, then Hit@k (all descending), then by
/// weights ascending so equal scores print in a stable order.
fn sort_sweep(rows: &mut [SweepRow]) {
    rows.sort_by(|a, b| {
        b.means
            .ndcg_at_k
            .total_cmp(&a.means.ndcg_at_k)
            .then_with(|| b.means.mrr.total_cmp(&a.means.mrr))
            .then_with(|| b.means.hit_at_k.total_cmp(&a.means.hit_at_k))
            .then_with(|| a.weights.dense.total_cmp(&b.weights.dense))
            .then_with(|| a.weights.sparse.total_cmp(&b.weights.sparse))
            .then_with(|| a.weights.bm25.total_cmp(&b.weights.bm25))
    });
}

/// Score every case under one fusion setting.
#[allow(clippy::too_many_arguments)]
fn score_cases(
    index: &SearchIndex,
    cases: &[LabelCase],
    case_ids: &[String],
    inputs: &[QueryInputs],
    top_k: usize,
    mode: Mode,
    options: RankingOptions,
    graded: bool,
) -> Result<Vec<CaseMetrics>> {
    let mut per_case = Vec::with_capacity(cases.len());
    for ((case, id), input) in cases.iter().zip(case_ids).zip(inputs) {
        let (pages, _) = run_query(index, input, &case.query, top_k, mode, options)?;
        let urls: Vec<String> = pages.into_iter().map(|p| p.url).collect();
        let retrieved: Vec<String> = urls.iter().map(|u| normalize_eval_url(u)).collect();
        let grades = case.grades();
        let relevant: Vec<String> = grades.iter().map(|(u, _)| u.clone()).collect();
        per_case.push(CaseMetrics {
            id: id.clone(),
            query: case.query.clone(),
            hit: hit_at_k(&retrieved, &relevant, top_k),
            rr: mrr(&retrieved, &relevant),
            ndcg: if graded {
                ndcg_graded_at_k(&retrieved, &grades, top_k)
            } else {
                ndcg_at_k(&retrieved, &relevant, top_k)
            },
            first_relevant_rank: retrieved
                .iter()
                .position(|u| relevant.contains(u))
                .map(|p| p + 1),
            top: urls,
        });
    }
    Ok(per_case)
}

fn cmd_eval(
    index_path: PathBuf,
    labels: PathBuf,
    top_k: usize,
    mode: Mode,
    lane: Option<&str>,
    json: bool,
    opts: EvalOptions,
) -> Result<()> {
    if top_k == 0 {
        bail!("--top-k must be > 0");
    }
    let raw = fs::read_to_string(&labels)
        .with_context(|| format!("reading labels {}", labels.display()))?;
    let set: LabelSet = toml::from_str(&raw)
        .with_context(|| format!("parsing labels {} as TOML", labels.display()))?;
    validate_label_cases(&set.cases)?;
    if opts.graded && set.cases.iter().all(|c| c.graded.is_empty()) {
        eprintln!("warning: --graded given but no case has a [cases.graded] table; nDCG is binary");
    }

    let index = load_index(&index_path)?;

    // Labels and page URLs are compared in canonical form (no host, no
    // trailing slash, lowercase); a label that names no page at all is
    // almost always a typo, so say so before the metrics print zeros.
    let page_urls: HashSet<String> = index
        .metadata
        .iter()
        .map(|m| normalize_eval_url(&m.url))
        .collect();
    let case_ids: Vec<String> = set
        .cases
        .iter()
        .enumerate()
        .map(|(i, c)| c.id.clone().unwrap_or_else(|| format!("case-{}", i + 1)))
        .collect();
    for (case, id) in set.cases.iter().zip(&case_ids) {
        for (url, _) in case.grades() {
            if !page_urls.contains(&url) {
                eprintln!(
                    "warning: {}: relevant url {:?} is not a page in the index",
                    id, url
                );
            }
        }
    }

    // --all-modes needs every arm loaded; hybrid loads them all.
    let load_mode = if opts.all_modes { Mode::Hybrid } else { mode };
    let runtime = QueryRuntime::with_options(&index, load_mode, lane, opts.ranking)?;
    // Queries are embedded once; every setting below re-fuses the same inputs.
    let mut inputs = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        inputs.push(runtime.inputs(&index, &case.query)?);
    }

    let per_case = score_cases(
        &index,
        &set.cases,
        &case_ids,
        &inputs,
        top_k,
        mode,
        opts.ranking,
        opts.graded,
    )?;
    let main = means(&per_case);

    let modes = if opts.all_modes {
        let mut rows = Vec::new();
        let mut list = vec![Mode::Hybrid];
        if !index.manifest.dense.is_empty() {
            list.push(Mode::Dense);
        }
        if index.sparse.is_some() {
            list.push(Mode::Sparse);
        }
        list.push(Mode::Keyword);
        for m in list {
            let cases = score_cases(
                &index,
                &set.cases,
                &case_ids,
                &inputs,
                top_k,
                m,
                opts.ranking,
                opts.graded,
            )?;
            rows.push(ModeRow {
                mode: m,
                means: means(&cases),
            });
        }
        Some(rows)
    } else {
        None
    };

    let sweep = if opts.sweep {
        let mut rows = Vec::new();
        for w in sweep_grid() {
            let options = RankingOptions {
                weights: Some(w),
                ..opts.ranking
            };
            let cases = score_cases(
                &index,
                &set.cases,
                &case_ids,
                &inputs,
                top_k,
                mode,
                options,
                opts.graded,
            )?;
            rows.push(SweepRow {
                weights: w,
                means: means(&cases),
            });
        }
        sort_sweep(&mut rows);
        Some(rows)
    } else {
        None
    };

    let report = EvalReport {
        k: top_k,
        mode,
        cases: per_case.len(),
        hit_at_k: main.hit_at_k,
        mrr: main.mrr,
        ndcg_at_k: main.ndcg_at_k,
        graded: opts.graded,
        weights: opts
            .ranking
            .weights
            .unwrap_or_else(|| Weights::for_index(&index)),
        fetch_k: opts
            .ranking
            .fetch_k
            .unwrap_or_else(|| eddie::search::fetch_k(top_k)),
        rrf_k: opts.ranking.rrf_k.unwrap_or(eddie::search::RRF_K),
        per_case,
        modes,
        sweep,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let ndcg_label = if opts.graded { "ndcg(g)" } else { "ndcg" };
    println!(
        "\n{:<24} {:>6} {:>6} {:>7}  first relevant",
        "case", "hit", "rr", ndcg_label
    );
    println!("{}", "-".repeat(60));
    for c in &report.per_case {
        println!(
            "{:<24} {:>6.2} {:>6.2} {:>7.2}  {}",
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
        "{} cases, mode {}: Hit@{} {:.3}  MRR {:.3}  nDCG@{} {:.3}  (weights {}/{}/{}, fetch_k {}, rrf_k {}{})",
        report.cases,
        report.mode.as_str(),
        report.k,
        report.hit_at_k,
        report.mrr,
        report.k,
        report.ndcg_at_k,
        report.weights.dense,
        report.weights.sparse,
        report.weights.bm25,
        report.fetch_k,
        report.rrf_k,
        if opts.graded { ", graded" } else { "" }
    );

    if let Some(rows) = &report.modes {
        println!(
            "\n{:<8} {:>7} {:>7} {:>8}",
            "mode", "hit", "mrr", ndcg_label
        );
        println!("{}", "-".repeat(34));
        for r in rows {
            println!(
                "{:<8} {:>7.3} {:>7.3} {:>8.3}",
                r.mode.as_str(),
                r.means.hit_at_k,
                r.means.mrr,
                r.means.ndcg_at_k
            );
        }
    }

    if let Some(rows) = &report.sweep {
        println!(
            "\nweight sweep ({} settings, mode {}, best first)",
            rows.len(),
            report.mode.as_str()
        );
        println!(
            "{:>5} {:>6} {:>5} {:>7} {:>7} {:>8}",
            "dense", "sparse", "bm25", "hit", "mrr", ndcg_label
        );
        println!("{}", "-".repeat(44));
        for r in rows {
            println!(
                "{:>5} {:>6} {:>5} {:>7.3} {:>7.3} {:>8.3}",
                r.weights.dense,
                r.weights.sparse,
                r.weights.bm25,
                r.means.hit_at_k,
                r.means.mrr,
                r.means.ndcg_at_k
            );
        }
        if let Some(best) = rows.first() {
            println!(
                "\nBest: --weights {},{},{}",
                best.weights.dense, best.weights.sparse, best.weights.bm25
            );
        }
    }
    Ok(())
}

/// Canonical form of a page URL for label matching: scheme and host
/// dropped, a leading slash ensured, the trailing slash trimmed, lowercased.
/// `/posts/Foo/`, `posts/foo` and `https://example.com/posts/foo` all
/// become `/posts/foo`.
fn normalize_eval_url(url: &str) -> String {
    let mut s = url.trim();
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        s = rest.find('/').map_or("/", |i| &rest[i..]);
    }
    let trimmed = s.trim_end_matches('/');
    let mut out = String::with_capacity(trimmed.len() + 1);
    if !trimmed.starts_with('/') {
        out.push('/');
    }
    out.push_str(trimmed);
    out.to_lowercase()
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
    if top_k == 0 {
        bail!("--top-k must be > 0");
    }
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
    if !interactive && suite.cases.is_empty() {
        bail!("no acceptance cases available. pass --eval or use --interactive to build one");
    }

    // The grid of indexes depends only on the corpus and the parameters, so
    // it is built once and re-scored as cases are added.
    let mut grid = TuneGrid::build(&docs, model_id, chunk_sizes, overlaps, mode)?;

    if interactive {
        interactive_collect_cases(&mut suite, &mut grid, top_k, mode)?;
        let persist_path = save_eval.or(eval);
        if let Some(path) = persist_path {
            write_suite(&path, &suite)?;
            eprintln!("Saved acceptance suite to {}", path.display());
        }
    }

    if suite.cases.is_empty() {
        bail!("no acceptance cases available. pass --eval or use --interactive to build one");
    }

    let candidates = run_tuning(&mut grid, &suite, top_k, mode)?;
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
    parser_for_with(cms, false)
}

fn parser_for_with(cms: Cms, include_noindex: bool) -> Box<dyn ContentParser> {
    match cms {
        Cms::Hugo => Box::new(HugoParser),
        Cms::Astro => Box::new(AstroParser),
        Cms::Docusaurus => Box::new(DocusaurusParser),
        Cms::Mkdocs => Box::new(MkDocsParser),
        Cms::Eleventy => Box::new(EleventyParser),
        Cms::Jekyll => Box::new(JekyllParser),
        Cms::Html => Box::new(HtmlParser::with_options(HtmlOptions { include_noindex })),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_qa_corpus(
    index_path: PathBuf,
    output: PathBuf,
    ollama_model: Option<String>,
    ollama_url: String,
    ollama_max_chunks: usize,
    ollama_max_pairs_per_chunk: usize,
    ollama_temperature: f32,
    qa_seed: Option<u64>,
    qa_heuristics: bool,
    qa_subject: Option<String>,
) -> Result<()> {
    eprintln!("Loading index from {}...", index_path.display());
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;
    if index.texts.is_empty() {
        bail!("index does not contain chunk texts. rebuild index with current eddie first");
    }

    let mut corpus = if !index.qa.is_empty() {
        eprintln!("Using embedded QA section from index...");
        QaCorpus {
            version: 1,
            entries: index.qa.clone(),
        }
    } else if qa_heuristics {
        let built = build_qa_corpus_from_chunks_with_subject(
            &index.texts,
            &index.metadata,
            qa_subject.as_deref().unwrap_or(eddie::qa::DEFAULT_SUBJECT),
        );
        eprintln!("Heuristic QA entries: {}", built.entries.len());
        built
    } else {
        if ollama_model.is_none() {
            eprintln!(
                "warning: the index has no qa section; without --qa-heuristics or --ollama-model the corpus will be empty"
            );
        }
        QaCorpus {
            version: 1,
            entries: Vec::new(),
        }
    };

    if let Some(model) = ollama_model {
        eprintln!("Running Ollama synthesis with model {}...", model);
        let cfg = OllamaConfig {
            model,
            endpoint: ollama_url,
            max_chunks: ollama_max_chunks,
            max_pairs_per_chunk: ollama_max_pairs_per_chunk,
            temperature: ollama_temperature,
            seed: qa_seed,
            subject: qa_subject.clone(),
            ..Default::default()
        };
        let llm_entries = synthesize_with_ollama_from_chunks(&index.texts, &index.metadata, &cfg)?;
        eprintln!("Ollama QA entries: {}", llm_entries.len());
        corpus.entries.extend(llm_entries);
    }
    corpus.dedup();

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
    claims_heuristics: bool,
) -> Result<()> {
    eprintln!("Loading index from {}...", index_path.display());
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;
    if index.texts.is_empty() {
        bail!("index does not contain chunk texts. rebuild index with current eddie first");
    }

    let mut corpus = if !index.claims.is_empty() {
        eprintln!("Using embedded claims section from index...");
        ClaimCorpus {
            version: 1,
            claims: index.claims.clone(),
        }
    } else if claims_heuristics {
        let built = build_claim_corpus_from_chunks(&index.texts, &index.metadata);
        eprintln!("Heuristic claims: {}", built.claims.len());
        built
    } else {
        if claims_edits.is_none() {
            eprintln!(
                "warning: the index has no claims section; without --claims-heuristics or --claims-edits the corpus will be empty"
            );
        }
        ClaimCorpus {
            version: 1,
            claims: Vec::new(),
        }
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

/// One `chunk_size × overlap` cell of the tuning grid: the index `eddie
/// index` would ship for those parameters.
struct TuneCell {
    chunk_size: usize,
    overlap: usize,
    index: SearchIndex,
}

/// Everything `eddie tune` needs to score a suite: the encoder, one index
/// per grid cell (chunked and embedded exactly once), and the query vectors
/// embedded so far. The interactive loop re-scores against this instead of
/// re-embedding the corpus after every case.
struct TuneGrid {
    embedder: Option<Box<dyn DenseEncoder>>,
    cells: Vec<TuneCell>,
    query_vectors: HashMap<String, Vec<f32>>,
}

impl TuneGrid {
    fn build(
        docs: &[Document],
        model_id: &str,
        chunk_sizes: &str,
        overlaps: &str,
        mode: Mode,
    ) -> Result<Self> {
        let chunk_values = parse_usize_csv(chunk_sizes)?;
        let overlap_values = parse_usize_csv(overlaps)?;
        if mode == Mode::Sparse {
            bail!("tune cannot build the sparse arm yet; use --mode hybrid, dense, or keyword");
        }
        let embedder = if matches!(mode, Mode::Dense | Mode::Hybrid) {
            eprintln!("Loading embedding model {} for tuning...", model_id);
            let device = select_device(DevicePref::Auto)?;
            Some(load_dense(model_id, &device, &DenseOverrides::default())?)
        } else {
            None
        };
        let mut cells = Vec::with_capacity(chunk_values.len() * overlap_values.len());
        for &chunk_size in &chunk_values {
            for &overlap in &overlap_values {
                eprintln!(
                    "Building index for chunk_size={}, overlap={}...",
                    chunk_size, overlap
                );
                let index = build_index_in_memory(docs, chunk_size, overlap, embedder.as_deref())?;
                cells.push(TuneCell {
                    chunk_size,
                    overlap,
                    index,
                });
            }
        }
        Ok(Self {
            embedder,
            cells,
            query_vectors: HashMap::new(),
        })
    }

    /// Query vector for `query`, embedded on first use and cached; `None`
    /// when the mode runs without a dense arm.
    fn query_vector(&mut self, query: &str) -> Result<Option<Vec<f32>>> {
        let Some(embedder) = &self.embedder else {
            return Ok(None);
        };
        if let Some(v) = self.query_vectors.get(query) {
            return Ok(Some(v.clone()));
        }
        let v = embed_query(embedder.as_ref(), query)?;
        self.query_vectors.insert(query.to_string(), v.clone());
        Ok(Some(v))
    }
}

fn run_tuning(
    grid: &mut TuneGrid,
    suite: &AcceptanceSuite,
    default_top_k: usize,
    mode: Mode,
) -> Result<Vec<TuneCandidate>> {
    let mut query_vectors = Vec::with_capacity(suite.cases.len());
    for case in &suite.cases {
        query_vectors.push(grid.query_vector(&case.query)?);
    }

    let mut candidates = Vec::with_capacity(grid.cells.len());
    for cell in &grid.cells {
        eprintln!(
            "Evaluating chunk_size={}, overlap={}...",
            cell.chunk_size, cell.overlap
        );
        let mut case_reports = Vec::new();
        for (case, query_vec) in suite.cases.iter().zip(&query_vectors) {
            let top_k = case.top_k.unwrap_or(default_top_k);
            let ids =
                retrieve_chunk_ids(&cell.index, &case.query, query_vec.as_deref(), top_k, mode)?;
            let context = build_eval_context(&cell.index, &ids);
            case_reports.push(evaluate_case(case, &context));
        }

        let summary = summarize(case_reports, suite);
        candidates.push(TuneCandidate {
            chunk_size: cell.chunk_size,
            overlap: cell.overlap,
            passed_cases: summary.passed_cases,
            total_cases: summary.total_cases,
            pass_rate: summary.pass_rate,
            weighted_score: summary.weighted_score,
            weighted_total: summary.weighted_total,
        });
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
/// dense lane, BM25, title context on), without writing it.
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
                let budget = chunk_budget(enc, chunk_size)?;
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
    // Same title/section prefix `eddie index` applies by default, so tune
    // measures what ships.
    let prefixes: Vec<String> = all_chunks.iter().map(|c| context_prefix(&c.meta)).collect();
    let index_texts: Vec<String> = all_chunks
        .iter()
        .zip(&prefixes)
        .map(|(c, p)| with_context(p, &c.text))
        .collect();

    let mut builder = IndexBuilder::new();
    if let Some(enc) = encoder {
        let inputs: Vec<String> = all_chunks
            .iter()
            .zip(&prefixes)
            .map(|(c, p)| with_context(p, &c.embed_text()))
            .collect();
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let vectors = embed_texts_with(enc, &refs, TextKind::Document, DEFAULT_BATCH_SIZE)?;
        let dim = enc.dim();
        builder.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(enc.spec().clone(), dim, n, &vectors, Quant::Int8)?,
        )?;
    }
    builder.add_chunks_indexed(metadata, texts, index_texts, overlap_words)?;
    builder.title_context(true);
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
        ..Query::default()
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

/// Wordpieces the lane's document prefix adds in front of every chunk
/// (special tokens excluded; the chunker counts those with the text).
fn doc_prefix_tokens(lane: &dyn DenseEncoder) -> usize {
    let prefix = &lane.spec().doc_prefix;
    if prefix.is_empty() {
        return 0;
    }
    lane.count_tokens(prefix)
        .saturating_sub(lane.count_tokens(""))
}

/// Token budget for one chunk: `chunk_size` capped at the lane's sequence
/// limit, minus the document prefix the encoder prepends, so a full chunk
/// still fits once prefixed and nothing is truncated at embedding time.
fn chunk_budget(lane: &dyn DenseEncoder, chunk_size: usize) -> Result<usize> {
    let spec = lane.spec();
    let prefix = doc_prefix_tokens(lane);
    let budget = chunk_size.min(spec.max_seq_len).saturating_sub(prefix);
    if budget == 0 {
        bail!(
            "chunk budget is 0: chunk size {} (capped at lane {:?} max_seq_len {}) leaves no room after the {}-token doc prefix {:?}",
            chunk_size,
            spec.id,
            spec.max_seq_len,
            prefix,
            spec.doc_prefix
        );
    }
    Ok(budget)
}

/// Names of the flags whose condition is set, for "ignored" warnings.
fn flags_given(flags: &[(bool, &'static str)]) -> Vec<&'static str> {
    flags
        .iter()
        .filter(|(given, _)| *given)
        .map(|(_, name)| *name)
        .collect()
}

/// Warn when a lane's manifest runtime cannot be honoured by the browser.
fn warn_about_lane_runtime(spec: &DenseSpec) {
    match &spec.runtime {
        RuntimeSpec::WasmCandle { files } => {
            let weights: Vec<&str> = files
                .iter()
                .map(String::as_str)
                .filter(|f| *f != "config.json" && *f != "tokenizer.json")
                .collect();
            if weights != ["model.safetensors"] {
                eprintln!(
                    "  warning: lane '{}' weights are {:?}; the browser WASM runtime loads only a single model.safetensors, so the widget will skip this lane. Pass --dense-runtime webgpu-onnx:<repo>:<dtype> or pick a repo that ships model.safetensors.",
                    spec.id, weights
                );
            }
        }
        RuntimeSpec::WebgpuOnnx { pooling, .. } => {
            let own = spec.pooling.transformers_name();
            if pooling != own {
                eprintln!(
                    "  warning: lane '{}' runtime pooling '{}' differs from the lane's own pooling '{}'; browser query vectors will not match the stored document vectors unless the ONNX graph already applies '{}' pooling",
                    spec.id, pooling, own, own
                );
            }
        }
    }
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

/// `wasm-candle`, `webgpu-onnx:<repo>:<dtype>[:<dtype_f16>[:<pooling>]]`, or
/// `auto`/`` for the registry default. An omitted pooling is left empty here
/// and filled in by `load_dense` from the lane's own pooling once the model
/// config has been read; only an explicit value overrides it.
fn parse_runtime_spec(spec: &str) -> Result<Option<RuntimeSpec>> {
    const USAGE: &str = "webgpu-onnx:<repo>:<dtype>[:<dtype_f16>[:<pooling>]]";
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if spec.eq_ignore_ascii_case("wasm-candle") {
        return Ok(Some(models::runtime_spec(models::RuntimeKind::WasmCandle)));
    }
    let Some(rest) = spec.strip_prefix("webgpu-onnx:") else {
        bail!("unknown --dense-runtime '{spec}' (expected wasm-candle or {USAGE})");
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 || parts.len() > 4 || parts[0].is_empty() || parts[1].is_empty() {
        bail!("--dense-runtime {USAGE}, got '{spec}'");
    }
    let segment = |i: usize| {
        parts
            .get(i)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    let pooling = segment(3).unwrap_or_default();
    if !pooling.is_empty() && !matches!(pooling.as_str(), "mean" | "cls" | "last_token" | "none") {
        bail!(
            "--dense-runtime pooling must be mean, cls, last_token or none (transformers.js names), got '{pooling}'"
        );
    }
    Ok(Some(RuntimeSpec::WebgpuOnnx {
        repo: parts[0].to_string(),
        dtype: parts[1].to_string(),
        dtype_f16: segment(2),
        pooling,
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
    grid: &mut TuneGrid,
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

        let candidates = run_tuning(grid, suite, top_k, mode)?;
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
    fn runtime_spec_pooling_is_explicit_or_deferred() {
        // No pooling segment: left empty for load_dense to derive from the lane.
        assert!(matches!(
            parse_runtime_spec("webgpu-onnx:org/repo:q8").unwrap(),
            Some(RuntimeSpec::WebgpuOnnx { pooling, dtype_f16: None, .. }) if pooling.is_empty()
        ));
        assert!(matches!(
            parse_runtime_spec("webgpu-onnx:org/repo:q4::cls").unwrap(),
            Some(RuntimeSpec::WebgpuOnnx { pooling, dtype_f16: None, .. }) if pooling == "cls"
        ));
        assert!(matches!(
            parse_runtime_spec("webgpu-onnx:org/repo:q4:q4f16:mean").unwrap(),
            Some(RuntimeSpec::WebgpuOnnx { pooling, dtype_f16: Some(f16), .. })
                if pooling == "mean" && f16 == "q4f16"
        ));
        assert!(parse_runtime_spec("webgpu-onnx:org/repo:q4:q4f16:average").is_err());
        assert!(parse_runtime_spec("webgpu-onnx:org/repo:q4:q4f16:mean:extra").is_err());
        assert!(parse_runtime_spec("webgpu-onnx:org/repo").is_err());
    }

    #[test]
    fn eval_urls_normalize_to_host_less_lowercase_paths() {
        assert_eq!(normalize_eval_url("/posts/Foo/"), "/posts/foo");
        assert_eq!(normalize_eval_url("posts/foo"), "/posts/foo");
        assert_eq!(
            normalize_eval_url("https://Example.com/posts/foo"),
            "/posts/foo"
        );
        assert_eq!(normalize_eval_url("http://example.com"), "/");
        assert_eq!(normalize_eval_url(" /about "), "/about");
        assert_eq!(normalize_eval_url("/"), "/");
        assert_eq!(normalize_eval_url(""), "/");
    }

    /// Minimal encoder whose tokenizer counts whitespace words plus two
    /// special tokens, enough to exercise the budget arithmetic.
    struct WordCounter(DenseSpec);

    impl DenseEncoder for WordCounter {
        fn spec(&self) -> &DenseSpec {
            &self.0
        }
        fn embed(&self, _: &[&str], _: TextKind) -> Result<Vec<Vec<f32>>> {
            unreachable!()
        }
        fn truncated_count(&self) -> usize {
            0
        }
        fn reset_truncated_count(&self) {}
        fn count_tokens(&self, text: &str) -> usize {
            text.split_whitespace().count() + 2
        }
    }

    #[test]
    fn chunk_budget_subtracts_the_doc_prefix() {
        let mut spec = eddie::embed::bert_spec_skeleton("intfloat/e5-small-v2");
        spec.max_seq_len = 512;
        assert_eq!(spec.doc_prefix, "passage: ");
        let lane = WordCounter(spec.clone());
        assert_eq!(doc_prefix_tokens(&lane), 1);
        assert_eq!(chunk_budget(&lane, 256).unwrap(), 255);
        assert_eq!(chunk_budget(&lane, 1024).unwrap(), 511);
        assert!(chunk_budget(&lane, 1).is_err());

        spec.doc_prefix = String::new();
        let lane = WordCounter(spec);
        assert_eq!(doc_prefix_tokens(&lane), 0);
        assert_eq!(chunk_budget(&lane, 256).unwrap(), 256);
        assert_eq!(chunk_budget(&lane, 1024).unwrap(), 512);
    }

    #[test]
    fn ranking_options_parse_and_validate() {
        let o = ranking_options(Some("1.2,0.8,0.6"), Some(40), Some(30.0)).unwrap();
        assert_eq!(
            o.weights,
            Some(Weights {
                dense: 1.2,
                sparse: 0.8,
                bm25: 0.6
            })
        );
        assert_eq!(o.fetch_k, Some(40));
        assert_eq!(o.rrf_k, Some(30.0));
        assert_eq!(
            ranking_options(None, None, None).unwrap(),
            RankingOptions::default()
        );
        assert!(ranking_options(Some("1,2"), None, None).is_err());
        assert!(ranking_options(None, Some(0), None).is_err());
        assert!(ranking_options(None, None, Some(-1.0)).is_err());
        assert!(ranking_options(None, None, Some(f64::INFINITY)).is_err());
    }

    #[test]
    fn sweep_grid_is_the_documented_48_settings_sorted_by_ndcg_then_mrr() {
        let grid = sweep_grid();
        assert_eq!(grid.len(), 48);
        assert_eq!(
            grid[0],
            Weights {
                dense: 0.8,
                sparse: 0.6,
                bm25: 0.6
            }
        );
        let row = |d: f64, ndcg: f64, mrr: f64| SweepRow {
            weights: Weights {
                dense: d,
                sparse: 1.0,
                bm25: 1.0,
            },
            means: Means {
                hit_at_k: 1.0,
                mrr,
                ndcg_at_k: ndcg,
            },
        };
        let mut rows = vec![row(1.2, 0.5, 0.9), row(0.8, 0.7, 0.4), row(1.0, 0.7, 0.6)];
        sort_sweep(&mut rows);
        let order: Vec<f64> = rows.iter().map(|r| r.weights.dense).collect();
        assert_eq!(order, vec![1.0, 0.8, 1.2]);
    }

    #[test]
    fn label_cases_accept_graded_tables_and_reject_bad_grades() {
        let set: LabelSet = toml::from_str(
            r#"
[[cases]]
query = "how long has jason been programming"
relevant = ["/skills/programming-languages/"]
[cases.graded]
"/skills/programming-languages/" = 3
"/r/" = 1

[[cases]]
id = "graded-only"
query = "rust"
[cases.graded]
"/Posts/Rust/" = 2
"#,
        )
        .unwrap();
        validate_label_cases(&set.cases).unwrap();
        let g = set.cases[0].grades();
        assert_eq!(
            g,
            vec![
                ("/r".to_string(), 1),
                ("/skills/programming-languages".to_string(), 3)
            ]
        );
        assert_eq!(set.cases[1].grades(), vec![("/posts/rust".to_string(), 2)]);

        let bad: LabelSet = toml::from_str(
            r#"
[[cases]]
query = "x"
[cases.graded]
"/a/" = 4
"#,
        )
        .unwrap();
        assert!(validate_label_cases(&bad.cases).is_err());
        let none: LabelSet = toml::from_str("[[cases]]\nquery = \"x\"\n").unwrap();
        assert!(validate_label_cases(&none.cases).is_err());
        assert!(validate_label_cases(&[]).is_err());
    }

    #[test]
    fn rating_weight_map() {
        assert_eq!(rating_weight(Some(1)), 2.0);
        assert_eq!(rating_weight(Some(5)), 1.0);
        assert_eq!(rating_weight(None), 1.0);
    }
}
