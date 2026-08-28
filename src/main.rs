// SPDX-License-Identifier: GPL-3.0-only

//! Eddie CLI: build-time indexer for static site content.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use eddie::chunk::{Chunk, ChunkMeta, ChunkStrategy, Document, chunk_document_with_strategy};
use eddie::claims::{
    ClaimEntry, apply_claim_edits, build_claim_corpus_from_chunks, parse_claim_edits_toml,
};
use eddie::embed::Embedder;
use eddie::eval::{
    AcceptanceCase, AcceptanceSuite, evaluate_case, load_suite, summarize, write_suite,
};
use eddie::index::{DenseLane, IndexBuilder, SCOPE_CHUNKS, SCOPE_CLAIMS, SCOPE_QA, SearchIndex};
use eddie::manifest::{DenseSpec, Family, Pooling, Quant, RuntimeSpec, SparseTerm, TextKind};
use eddie::parse::{
    AstroParser, ContentParser, DocusaurusParser, EleventyParser, HugoParser, JekyllParser,
    MkDocsParser, parse_content_dir,
};
use eddie::qa::{
    OllamaConfig, OpenRouterConfig, QaCorpus, QaEntry, build_qa_corpus_from_chunks,
    build_qa_entries_from_chunks, synthesize_with_ollama_from_chunks,
    synthesize_with_openrouter_from_chunks,
};
use eddie::search::{
    Mode, PageResult, Query, Retrieval, Weights, group_pages, query_terms, retrieve,
    sparse_query_terms_local,
};

const DEFAULT_MODEL: &str = "sentence-transformers/multi-qa-MiniLM-L6-cos-v1";

#[derive(Parser)]
#[command(name = "eddie", about = "Semantic search indexer for static sites")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
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

        /// HuggingFace model ID for embeddings.
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

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

        /// Dense lane id to embed the query with (default: the index's first wasm-candle lane).
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

        /// Dense lane id (default: the index's first wasm-candle lane).
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
            model,
            chunk_size,
            overlap,
            chunk_strategy,
            coarse_chunk_size,
            coarse_overlap,
            summary_lane,
            qa,
            claims,
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
            &model,
            chunk_size,
            overlap,
            chunk_strategy,
            coarse_chunk_size,
            coarse_overlap,
            summary_lane,
            qa,
            claims,
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

fn cmd_index(
    content_dir: PathBuf,
    cms: Cms,
    output: PathBuf,
    model_id: &str,
    chunk_size: usize,
    overlap: usize,
    chunk_strategy: ChunkingStrategy,
    coarse_chunk_size: Option<usize>,
    coarse_overlap: Option<usize>,
    summary_lane: bool,
    qa_enabled: bool,
    claims_enabled: bool,
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
    let docs = parse_content_dir(&content_dir, parser.as_ref())?;
    eprintln!("  Found {} documents", docs.len());

    // Chunk documents
    eprintln!("Chunking documents (strategy: {:?})...", chunk_strategy);
    let mut all_chunks = Vec::new();
    let strategy = match chunk_strategy {
        ChunkingStrategy::Heading => ChunkStrategy::Heading,
        ChunkingStrategy::Semantic => ChunkStrategy::Semantic,
    };

    for doc in &docs {
        let mut fine = chunk_document_with_strategy(doc, chunk_size, overlap, strategy);
        for chunk in &mut fine {
            chunk.meta.granularity = Some("fine".to_string());
        }
        all_chunks.extend(fine);

        if let Some(coarse_size) = coarse_chunk_size {
            let coarse_overlap = coarse_overlap.unwrap_or(overlap);
            let mut coarse =
                chunk_document_with_strategy(doc, coarse_size, coarse_overlap, strategy);
            for chunk in &mut coarse {
                chunk.meta.granularity = Some("coarse".to_string());
            }
            all_chunks.extend(coarse);
        }

        if summary_lane {
            if let Some(summary_chunk) = build_summary_chunk(doc) {
                all_chunks.push(summary_chunk);
            }
        }
    }
    eprintln!("  Created {} chunks", all_chunks.len());

    // Keep factual extraction stable even when retrieval chunking is semantic/coarse.
    let mut fact_chunks = Vec::new();
    for doc in &docs {
        let mut chunks =
            chunk_document_with_strategy(doc, chunk_size, overlap, ChunkStrategy::Heading);
        for chunk in &mut chunks {
            chunk.meta.granularity = Some("facts".to_string());
        }
        fact_chunks.extend(chunks);
    }
    let fact_metadata: Vec<_> = fact_chunks.iter().map(|c| c.meta.clone()).collect();
    let fact_texts: Vec<String> = fact_chunks.iter().map(|c| c.text.clone()).collect();

    // Load embedding model
    eprintln!("Loading embedding model: {}...", model_id);
    let embedder = Embedder::new(model_id)?;
    eprintln!("  Embedding dimension: {}", embedder.dim());

    // Embed all chunks
    eprintln!("Embedding {} chunks...", all_chunks.len());
    let embed_inputs: Vec<String> = all_chunks.iter().map(|c| c.embed_text()).collect();
    let embed_refs: Vec<&str> = embed_inputs.iter().map(String::as_str).collect();
    let all_embeddings = embed_texts(&embedder, &embed_refs)?;
    let texts: Vec<&str> = all_chunks.iter().map(|c| c.text.as_str()).collect();

    // Build optional QA/claims sections
    let metadata: Vec<_> = all_chunks.iter().map(|c| c.meta.clone()).collect();
    let chunk_texts: Vec<String> = all_chunks.iter().map(|c| c.text.clone()).collect();
    let mut qa_entries: Vec<QaEntry> = Vec::new();
    let mut claims: Vec<ClaimEntry> = Vec::new();

    if qa_enabled {
        eprintln!("Building QA section...");
        qa_entries = build_qa_entries_from_chunks(&fact_texts, &fact_metadata);
        eprintln!("  Heuristic QA entries: {}", qa_entries.len());
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
                ..Default::default()
            };
            eprintln!("  Running OpenRouter QA synthesis...");
            let llm_entries =
                synthesize_with_openrouter_from_chunks(&fact_texts, &fact_metadata, &cfg)?;
            eprintln!("  OpenRouter QA entries: {}", llm_entries.len());
            qa_entries.extend(llm_entries);
            let mut corpus = QaCorpus {
                version: 1,
                entries: qa_entries,
            };
            corpus.dedup();
            qa_entries = corpus.entries;
        } else if let Some(model) = qa_ollama_model {
            let cfg = OllamaConfig {
                model,
                endpoint: qa_ollama_url,
                max_chunks: qa_ollama_max_chunks,
                max_pairs_per_chunk: qa_ollama_max_pairs_per_chunk,
                temperature: qa_ollama_temperature,
                ..Default::default()
            };
            eprintln!("  Running Ollama QA synthesis...");
            let llm_entries =
                synthesize_with_ollama_from_chunks(&fact_texts, &fact_metadata, &cfg)?;
            eprintln!("  Ollama QA entries: {}", llm_entries.len());
            qa_entries.extend(llm_entries);
            let mut corpus = QaCorpus {
                version: 1,
                entries: qa_entries,
            };
            corpus.dedup();
            qa_entries = corpus.entries;
        }
    }

    if claims_enabled {
        eprintln!("Building claims section...");
        let mut corpus = build_claim_corpus_from_chunks(&fact_texts, &fact_metadata);
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
    // TODO(integrator): the `--summary-lane` chunk built above by
    // `build_summary_chunk` is the page's first four sentences, which
    // duplicates fine chunk 0 and games BM25 length normalisation
    // (adversarial review). Drop the lane or restrict it to
    // `doc.meta.description` when the chunking region is reworked.
    let dim = embedder.dim();
    let spec = dense_spec_for_model(model_id, dim);
    let n = metadata.len();
    // TODO(integrator): once `Chunk` carries `overlap`, pass
    // `word_count(chunk.overlap)` here (texts as embedded, overlap prefix
    // included); the builder strips the prefix from the stored text.
    let overlap_words: Vec<u16> = vec![0; n];

    let mut builder = IndexBuilder::new();
    builder.add_chunks(metadata, chunk_texts, overlap_words)?;
    builder.add_dense_lane(
        SCOPE_CHUNKS,
        DenseLane::from_f32(spec.clone(), dim, n, &all_embeddings, Quant::Int8)?,
    )?;
    drop(all_embeddings);
    // TODO(integrator): `--sparse` / `--sparse-model`: encode `chunk_texts`
    // with `sparse::SparseDocEncoder` and call
    // `builder.add_sparse(&docs, encoder.idf(), SparseSpec { .. })`.

    if !qa_entries.is_empty() {
        eprintln!("Embedding QA section ({} entries)...", qa_entries.len());
        let qa_texts: Vec<String> = qa_entries
            .iter()
            .map(|q| format!("Q: {} A: {}", q.question, q.answer))
            .collect();
        let refs: Vec<&str> = qa_texts.iter().map(String::as_str).collect();
        let vectors = embed_texts(&embedder, &refs)?;
        builder.add_dense_lane(
            SCOPE_QA,
            DenseLane::from_f32(spec.clone(), dim, qa_entries.len(), &vectors, Quant::Int8)?,
        )?;
        builder.add_qa(qa_entries);
    }

    if !claims.is_empty() {
        eprintln!("Embedding claims section ({} claims)...", claims.len());
        let claim_texts: Vec<String> = claims
            .iter()
            .map(|c| format!("{} {} {} {}", c.subject, c.predicate, c.object, c.evidence))
            .collect();
        let refs: Vec<&str> = claim_texts.iter().map(String::as_str).collect();
        let vectors = embed_texts(&embedder, &refs)?;
        builder.add_dense_lane(
            SCOPE_CLAIMS,
            DenseLane::from_f32(spec.clone(), dim, claims.len(), &vectors, Quant::Int8)?,
        )?;
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

/// Lane description for a sentence-transformers BERT model loaded through the
/// current `Embedder` (mean pooling, L2-normalised, no prefixes).
// TODO(integrator): replace with the `models.rs` registry entry for
// `model_id` (pooling, max_seq_len, prefixes, revision, runtime files).
fn dense_spec_for_model(model_id: &str, dim: usize) -> DenseSpec {
    let short = model_id
        .rsplit('/')
        .next()
        .unwrap_or(model_id)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    DenseSpec {
        id: short,
        model: model_id.to_string(),
        family: Family::Bert,
        dim,
        pooling: Pooling::Mean,
        normalize: true,
        query_prefix: String::new(),
        doc_prefix: String::new(),
        max_seq_len: 512,
        revision: None,
        quant: Quant::Int8,
        runtime: RuntimeSpec::WasmCandle {
            files: vec![
                "config.json".to_string(),
                "tokenizer.json".to_string(),
                "model.safetensors".to_string(),
            ],
        },
    }
}

fn build_summary_chunk(doc: &Document) -> Option<Chunk> {
    let sentences = split_sentences_for_summary(&doc.body);
    if sentences.is_empty() {
        return None;
    }

    let mut picked = Vec::new();
    for sentence in sentences {
        if sentence.len() < 30 {
            continue;
        }
        picked.push(sentence.trim().to_string());
        if picked.len() >= 4 {
            break;
        }
    }

    if picked.is_empty() {
        return None;
    }

    Some(Chunk {
        text: picked.join(" "),
        overlap: String::new(),
        meta: ChunkMeta {
            title: doc.meta.title.clone(),
            url: doc.meta.url.clone(),
            section: None,
            date: doc.meta.date.clone(),
            granularity: Some("summary".to_string()),
            chunk_index: 0,
        },
    })
}

fn split_sentences_for_summary(text: &str) -> Vec<&str> {
    let splitter = regex::Regex::new(r"[\n\.!?]+\s*").unwrap();
    splitter
        .split(text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Embeds queries with one of the index's own dense lanes.
struct QueryEmbedder {
    lane: usize,
    spec: DenseSpec,
    embedder: Embedder,
}

impl QueryEmbedder {
    /// Pick `lane_id` (or the first wasm-candle lane) and load its model.
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
            None => index
                .manifest
                .first_wasm_lane()
                .with_context(|| {
                    format!(
                        "index has only webgpu lanes ({}); the CLI can embed queries only for wasm-candle (BERT) lanes",
                        lane_list(index)
                    )
                })?
                .clone(),
        };
        if !matches!(spec.runtime, RuntimeSpec::WasmCandle { .. }) {
            bail!(
                "lane {:?} is a webgpu-onnx lane; the CLI can embed queries only for wasm-candle (BERT) lanes",
                spec.id
            );
        }
        let lane = index
            .dense_lane(&spec.id)
            .with_context(|| format!("index has no dense section for lane {:?}", spec.id))?;
        eprintln!(
            "Loading embedding model for lane {}: {}...",
            spec.id, spec.model
        );
        // TODO(integrator): use `embed::load_dense(&spec.model, device, ..)` with
        // `spec.revision` so the CLI resolves the same pinned files as the worker.
        let embedder = Embedder::new(&spec.model)?;
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
        embed_query(&self.embedder, &self.spec, text)
    }
}

/// One query embedding with the lane's query prefix applied.
// TODO(integrator): swap `Embedder` for `DenseEncoder::embed(&[text], TextKind::Query)`.
fn embed_query(embedder: &Embedder, spec: &DenseSpec, text: &str) -> Result<Vec<f32>> {
    let prefixed = spec.prefixed(TextKind::Query, text);
    let mut vecs = embedder.embed_batch(&[prefixed.as_str()])?;
    vecs.pop().context("embedder returned no vector")
}

fn lane_list(index: &SearchIndex) -> String {
    index
        .manifest
        .dense
        .iter()
        .map(|d| {
            let kind = match d.runtime {
                RuntimeSpec::WasmCandle { .. } => "wasm-candle",
                RuntimeSpec::WebgpuOnnx { .. } => "webgpu-onnx",
            };
            format!("{} [{}]", d.id, kind)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fetch the sparse arm's `tokenizer.json` from HuggingFace (pinned to the
/// manifest revision). Returns `None`, with a warning, when it cannot be
/// loaded so the search degrades instead of failing.
fn load_sparse_tokenizer(index: &SearchIndex) -> Option<tokenizers::Tokenizer> {
    let spec = index.manifest.sparse.as_ref()?;
    let fetch = || -> Result<tokenizers::Tokenizer> {
        let api = hf_hub::api::sync::Api::new().context("creating HuggingFace Hub API client")?;
        let repo = api.repo(hf_hub::Repo::with_revision(
            spec.tokenizer.clone(),
            hf_hub::RepoType::Model,
            spec.revision.clone().unwrap_or_else(|| "main".to_string()),
        ));
        let path = repo
            .get("tokenizer.json")
            .with_context(|| format!("downloading tokenizer.json from {}", spec.tokenizer))?;
        tokenizers::Tokenizer::from_file(&path).map_err(|e| anyhow::anyhow!("{}", e))
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
        let dense = if matches!(mode, Mode::Hybrid | Mode::Dense) {
            Some(QueryEmbedder::for_index(index, lane)?)
        } else {
            None
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
        })
    }

    fn sparse_terms(&self, index: &SearchIndex, text: &str) -> Result<Option<Vec<SparseTerm>>> {
        match (&index.sparse, &self.sparse_tokenizer) {
            (Some(sparse), Some(tok)) => Ok(Some(sparse_query_terms_local(
                tok,
                &|id| sparse.idf_of(id),
                text,
            )?)),
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

// TODO(integrator): the synthesis workstream adds `hit_at_k`, `mrr`,
// `ndcg_at_k` to `eval.rs`; delete these three and import them.
/// 1.0 when any relevant url appears in the first `k` results.
fn hit_at_k(ranked: &[String], relevant: &[String], k: usize) -> f64 {
    if ranked.iter().take(k).any(|u| relevant.contains(u)) {
        1.0
    } else {
        0.0
    }
}

/// Reciprocal rank of the first relevant result (0 when none).
fn mrr(ranked: &[String], relevant: &[String]) -> f64 {
    ranked
        .iter()
        .position(|u| relevant.contains(u))
        .map(|p| 1.0 / (p as f64 + 1.0))
        .unwrap_or(0.0)
}

/// Binary-gain nDCG over the first `k` results.
fn ndcg_at_k(ranked: &[String], relevant: &[String], k: usize) -> f64 {
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, u)| relevant.contains(u))
        .map(|(i, _)| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    let ideal_hits = relevant.len().min(k);
    let idcg: f64 = (0..ideal_hits)
        .map(|i| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    if idcg == 0.0 { 0.0 } else { dcg / idcg }
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
        Some(Embedder::new(model_id)?)
    } else {
        None
    };
    if mode == Mode::Sparse {
        bail!("tune cannot build the sparse arm yet; use --mode hybrid, dense, or keyword");
    }

    let query_embeddings: Option<Vec<Vec<f32>>> = match &embedder {
        Some(embedder) => {
            let spec = dense_spec_for_model(model_id, embedder.dim());
            let mut rows = Vec::with_capacity(suite.cases.len());
            for case in &suite.cases {
                rows.push(embed_query(embedder, &spec, &case.query)?);
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
            let index =
                build_index_in_memory(docs, chunk_size, overlap, embedder.as_ref(), model_id)?;

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
    embedder: Option<&Embedder>,
    model_id: &str,
) -> Result<SearchIndex> {
    // TODO(integrator): `tune` still hard-codes heading chunking and the fine
    // lane; accept the same `--chunk-strategy` / coarse flags as `index`.
    let mut all_chunks = Vec::new();
    for doc in docs {
        let mut chunks =
            chunk_document_with_strategy(doc, chunk_size, overlap, ChunkStrategy::Heading);
        for chunk in &mut chunks {
            chunk.meta.granularity = Some("fine".to_string());
        }
        all_chunks.extend(chunks);
    }

    let metadata: Vec<_> = all_chunks.iter().map(|c| c.meta.clone()).collect();
    let texts: Vec<String> = all_chunks.iter().map(|c| c.text.clone()).collect();
    let n = texts.len();
    // TODO(integrator): pass the real overlap word counts once `Chunk` carries them.
    let overlap_words = vec![0u16; n];

    let mut builder = IndexBuilder::new();
    if let Some(embedder) = embedder {
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let vectors = embed_texts(embedder, &text_refs)?;
        let dim = embedder.dim();
        builder.add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(
                dense_spec_for_model(model_id, dim),
                dim,
                n,
                &vectors,
                Quant::Int8,
            )?,
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

fn embed_texts(embedder: &Embedder, texts: &[&str]) -> Result<Vec<f32>> {
    let mut out = Vec::new();
    let batch_size = 32;

    for (i, batch) in texts.chunks(batch_size).enumerate() {
        let vecs = embedder.embed_batch(batch)?;
        for vec in vecs {
            out.extend(vec);
        }
        if (i + 1) % 10 == 0 || (i + 1) * batch_size >= texts.len() {
            eprintln!(
                "  Embedded {}/{} chunks",
                ((i + 1) * batch_size).min(texts.len()),
                texts.len()
            );
        }
    }

    Ok(out)
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
    fn rating_weight_map() {
        assert_eq!(rating_weight(Some(1)), 2.0);
        assert_eq!(rating_weight(Some(5)), 1.0);
        assert_eq!(rating_weight(None), 1.0);
    }
}
