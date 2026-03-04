// SPDX-License-Identifier: GPL-3.0-only

//! Eddie CLI: build-time indexer for static site content.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use eddie::bm25::{Bm25Index, hybrid_rrf};
use eddie::chunk::{Chunk, ChunkMeta, ChunkStrategy, Document, chunk_document_with_strategy};
use eddie::claims::{
    ClaimEntry, apply_claim_edits, build_claim_corpus_from_chunks, parse_claim_edits_toml,
};
use eddie::embed::Embedder;
use eddie::eval::{
    AcceptanceCase, AcceptanceSuite, evaluate_case, load_suite, summarize, write_suite,
};
use eddie::index::SearchIndex;
use eddie::parse::{HugoParser, parse_content_dir};
use eddie::qa::{
    OllamaConfig, OpenRouterConfig, QaCorpus, QaEntry, build_qa_corpus_from_chunks,
    build_qa_entries_from_chunks, synthesize_with_ollama_from_chunks,
    synthesize_with_openrouter_from_chunks,
};
use eddie::search::search;

const DEFAULT_MODEL: &str = "sentence-transformers/all-MiniLM-L6-v2";

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

        /// Number of results to return.
        #[arg(long, default_value = "5")]
        top_k: usize,

        /// HuggingFace model ID (must match the model used during indexing).
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

        /// Search mode: semantic, keyword, or hybrid (default).
        #[arg(long, default_value = "hybrid")]
        mode: SearchMode,

        /// Which lanes to search: chunks, qa, claims, or all.
        #[arg(long, default_value = "all")]
        scope: SearchScope,
    },

    /// Tune chunking parameters against a site-owned acceptance suite.
    Tune {
        /// Path to content directory.
        #[arg(long)]
        content_dir: PathBuf,

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
    Semantic,
    Keyword,
    Hybrid,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SearchScope {
    Chunks,
    Qa,
    Claims,
    All,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ChunkingStrategy {
    Heading,
    Semantic,
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
            model,
            mode,
            scope,
        } => cmd_search(index, &query, top_k, &model, mode, scope),
        Command::Tune {
            content_dir,
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
    eprintln!("Parsing content from {}...", content_dir.display());
    let parser = HugoParser;
    let docs = parse_content_dir(&content_dir, &parser)?;
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
    let texts: Vec<&str> = all_chunks.iter().map(|c| c.text.as_str()).collect();
    let all_embeddings = embed_texts(&embedder, &texts)?;

    // Build BM25 index
    eprintln!("Building BM25 keyword index...");
    let bm25 = Bm25Index::build(&texts);

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

    // Embed QA and claim sections (same model as chunk embeddings)
    let (qa_dim, qa_embeddings) = if !qa_entries.is_empty() {
        eprintln!("Embedding QA section...");
        let qa_texts: Vec<String> = qa_entries
            .iter()
            .map(|q| format!("Q: {} A: {}", q.question, q.answer))
            .collect();
        let refs: Vec<&str> = qa_texts.iter().map(String::as_str).collect();
        (embedder.dim(), embed_texts(&embedder, &refs)?)
    } else {
        (0usize, Vec::new())
    };

    let (claim_dim, claim_embeddings) = if !claims.is_empty() {
        eprintln!("Embedding claims section...");
        let claim_texts: Vec<String> = claims
            .iter()
            .map(|c| format!("{} {} {} {}", c.subject, c.predicate, c.object, c.evidence))
            .collect();
        let refs: Vec<&str> = claim_texts.iter().map(String::as_str).collect();
        (embedder.dim(), embed_texts(&embedder, &refs)?)
    } else {
        (0usize, Vec::new())
    };

    // Build and write index
    let index = SearchIndex::new(
        model_id.to_string(),
        embedder.dim(),
        metadata,
        all_embeddings,
        bm25,
        chunk_texts,
    )
    .with_qa_section(qa_entries, qa_dim, qa_embeddings)
    .with_claims_section(claims, claim_dim, claim_embeddings);

    eprintln!("Writing index to {}...", output.display());
    let file = fs::File::create(&output)
        .with_context(|| format!("creating output file {}", output.display()))?;
    let writer = BufWriter::new(file);
    let is_ed_output = output
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ed"));
    if is_ed_output {
        index.write_ed_to(writer)?;
    } else {
        index.write_to(writer)?;
    }

    eprintln!(
        "Done! Index contains {} chunks, {} qa entries, {} claims.",
        all_chunks.len(),
        index.qa_entries.len(),
        index.claims.len()
    );
    Ok(())
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

fn cmd_search(
    index_path: PathBuf,
    query: &str,
    top_k: usize,
    model_id: &str,
    mode: SearchMode,
    scope: SearchScope,
) -> Result<()> {
    // Load index
    eprintln!("Loading index from {}...", index_path.display());
    let bytes = fs::read(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::from_bytes(&bytes)?;
    eprintln!(
        "  {} chunks, {} dimensions",
        index.metadata.len(),
        index.dim
    );

    let query_vec = if matches!(mode, SearchMode::Semantic | SearchMode::Hybrid) {
        eprintln!("Loading embedding model: {}...", model_id);
        let embedder = Embedder::new(model_id)?;
        let query_vecs = embedder.embed_batch(&[query])?;
        Some(query_vecs[0].clone())
    } else {
        None
    };

    if matches!(scope, SearchScope::Chunks | SearchScope::All) {
        let ids = retrieve_chunk_ids(&index, query, query_vec.as_deref(), top_k, mode)?;
        match mode {
            SearchMode::Semantic => println!("\nChunk semantic results for: \"{}\"", query),
            SearchMode::Keyword => println!("\nChunk keyword results for: \"{}\"", query),
            SearchMode::Hybrid => println!("\nChunk hybrid results for: \"{}\"", query),
        }
        println!("{}", "-".repeat(60));
        for (rank, chunk_idx) in ids.iter().enumerate() {
            let meta = &index.metadata[*chunk_idx];
            println!("{}. {} — {}", rank + 1, meta.title, meta.url);
            if let Some(section) = &meta.section {
                println!("   Section: {}", section);
            }
            if let Some(text) = index.texts.get(*chunk_idx) {
                let snippet = text.chars().take(180).collect::<String>();
                println!("   {}", snippet.replace('\n', " "));
            }
        }
    }

    if matches!(scope, SearchScope::Qa | SearchScope::All) {
        println!("\nQA lane results:");
        println!("{}", "-".repeat(60));
        if index.qa_entries.is_empty() || index.qa_embeddings.is_empty() {
            println!("(no QA section embedded in index)");
        } else if let Some(qvec) = query_vec.as_deref() {
            let hits = semantic_top_n(&index.qa_embeddings, index.qa_dim, qvec, top_k);
            for (rank, (idx, score)) in hits.into_iter().enumerate() {
                let item = &index.qa_entries[idx];
                println!("{}. [{:.4}] {}", rank + 1, score, item.question);
                println!("   {}", item.answer);
                println!("   source: {}", item.source_url);
            }
        } else {
            println!("(QA semantic lane requires semantic/hybrid mode)");
        }
    }

    if matches!(scope, SearchScope::Claims | SearchScope::All) {
        println!("\nClaims lane results:");
        println!("{}", "-".repeat(60));
        if index.claims.is_empty() || index.claim_embeddings.is_empty() {
            println!("(no claims section embedded in index)");
        } else if let Some(qvec) = query_vec.as_deref() {
            let hits = semantic_top_n(&index.claim_embeddings, index.claim_dim, qvec, top_k);
            for (rank, (idx, score)) in hits.into_iter().enumerate() {
                let c = &index.claims[idx];
                println!(
                    "{}. [{:.4}] {} {} {}",
                    rank + 1,
                    score,
                    c.subject,
                    c.predicate,
                    c.object
                );
                println!("   evidence: {}", c.evidence);
                println!("   source: {}", c.source_url);
            }
        } else {
            println!("(claims semantic lane requires semantic/hybrid mode)");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_tune(
    content_dir: PathBuf,
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
    let parser = HugoParser;
    eprintln!("Parsing content from {}...", content_dir.display());
    let docs = parse_content_dir(&content_dir, &parser)?;
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

    let mut corpus = if !index.qa_entries.is_empty() {
        eprintln!("Using embedded QA section from index...");
        QaCorpus {
            version: 1,
            entries: index.qa_entries.clone(),
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
    mode: SearchMode,
) -> Result<Vec<TuneCandidate>> {
    let chunk_values = parse_usize_csv(chunk_sizes)?;
    let overlap_values = parse_usize_csv(overlaps)?;

    let queries: Vec<&str> = suite.cases.iter().map(|c| c.query.as_str()).collect();
    let embedder = if matches!(mode, SearchMode::Semantic | SearchMode::Hybrid) {
        eprintln!("Loading embedding model {} for tuning...", model_id);
        Some(Embedder::new(model_id)?)
    } else {
        None
    };

    let query_embeddings = if let Some(embedder) = &embedder {
        Some(embedder.embed_batch(&queries)?)
    } else {
        None
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
            .partial_cmp(&a.weighted_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.pass_rate
                    .partial_cmp(&a.pass_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.chunk_size.cmp(&b.chunk_size))
            .then_with(|| a.overlap.cmp(&b.overlap))
    });

    Ok(candidates)
}

fn build_index_in_memory(
    docs: &[Document],
    chunk_size: usize,
    overlap: usize,
    embedder: Option<&Embedder>,
    model_id: &str,
) -> Result<SearchIndex> {
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
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let bm25 = Bm25Index::build(&text_refs);

    let (dim, embeddings) = if let Some(embedder) = embedder {
        (embedder.dim(), embed_texts(embedder, &text_refs)?)
    } else {
        (0usize, Vec::new())
    };

    Ok(SearchIndex::new(
        model_id.to_string(),
        dim,
        metadata,
        embeddings,
        bm25,
        texts,
    ))
}

fn retrieve_chunk_ids(
    index: &SearchIndex,
    query: &str,
    query_vec: Option<&[f32]>,
    top_k: usize,
    mode: SearchMode,
) -> Result<Vec<usize>> {
    let ids = match mode {
        SearchMode::Semantic => {
            let vec = query_vec.context("semantic mode requires query embedding")?;
            search(index, vec, top_k)
                .into_iter()
                .map(|r| r.chunk_index)
                .collect::<Vec<_>>()
        }
        SearchMode::Keyword => index
            .bm25
            .search(query, top_k)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        SearchMode::Hybrid => {
            let vec = query_vec.context("hybrid mode requires query embedding")?;
            let fetch_k = top_k.saturating_mul(3).max(top_k);
            let semantic = search(index, vec, fetch_k)
                .into_iter()
                .map(|r| (r.chunk_index, r.score))
                .collect::<Vec<_>>();
            let keyword = index.bm25.search(query, fetch_k);
            hybrid_rrf(&semantic, &keyword, top_k)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        }
    };
    Ok(dedupe_ids(ids))
}

fn semantic_top_n(flat: &[f32], dim: usize, query_vec: &[f32], top_k: usize) -> Vec<(usize, f32)> {
    if dim == 0 || flat.is_empty() || query_vec.len() != dim {
        return Vec::new();
    }
    let rows = flat.len() / dim;
    let mut scored = Vec::with_capacity(rows);
    for row in 0..rows {
        let start = row * dim;
        let emb = &flat[start..start + dim];
        let score = emb
            .iter()
            .zip(query_vec.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>();
        scored.push((row, score));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    scored
}

fn dedupe_ids(ids: Vec<usize>) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in ids {
        if seen.insert(id) {
            out.push(id);
        }
    }
    out
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
    mode: SearchMode,
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
