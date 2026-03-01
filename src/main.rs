// SPDX-License-Identifier: GPL-3.0-only

//! static-agent-cli: build-time indexer for static site content.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use static_agent::bm25::{hybrid_rrf, Bm25Index};
use static_agent::chunk::chunk_document;
use static_agent::embed::Embedder;
use static_agent::index::SearchIndex;
use static_agent::parse::{parse_content_dir, HugoParser};
use static_agent::search::search;

const DEFAULT_MODEL: &str = "sentence-transformers/all-MiniLM-L6-v2";

#[derive(Parser)]
#[command(name = "static-agent-cli", about = "Semantic search indexer for static sites")]
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
        #[arg(long, default_value = "index.bin")]
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
    },
}

#[derive(Clone, clap::ValueEnum)]
enum SearchMode {
    Semantic,
    Keyword,
    Hybrid,
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
        } => cmd_index(content_dir, output, &model, chunk_size, overlap),
        Command::Search {
            index,
            query,
            top_k,
            model,
            mode,
        } => cmd_search(index, &query, top_k, &model, mode),
    }
}

fn cmd_index(
    content_dir: PathBuf,
    output: PathBuf,
    model_id: &str,
    chunk_size: usize,
    overlap: usize,
) -> Result<()> {
    // Parse content
    eprintln!("Parsing content from {}...", content_dir.display());
    let parser = HugoParser;
    let docs = parse_content_dir(&content_dir, &parser)?;
    eprintln!("  Found {} documents", docs.len());

    // Chunk documents
    eprintln!("Chunking documents...");
    let mut all_chunks = Vec::new();
    for doc in &docs {
        let chunks = chunk_document(doc, chunk_size, overlap);
        all_chunks.extend(chunks);
    }
    eprintln!("  Created {} chunks", all_chunks.len());

    // Load embedding model
    eprintln!("Loading embedding model: {}...", model_id);
    let embedder = Embedder::new(model_id)?;
    eprintln!("  Embedding dimension: {}", embedder.dim());

    // Embed all chunks
    eprintln!("Embedding {} chunks...", all_chunks.len());
    let texts: Vec<&str> = all_chunks.iter().map(|c| c.text.as_str()).collect();
    let mut all_embeddings = Vec::new();
    let batch_size = 32;
    for (i, batch) in texts.chunks(batch_size).enumerate() {
        let vecs = embedder.embed_batch(batch)?;
        for vec in vecs {
            all_embeddings.extend(vec);
        }
        if (i + 1) % 10 == 0 || (i + 1) * batch_size >= texts.len() {
            eprintln!(
                "  Embedded {}/{} chunks",
                ((i + 1) * batch_size).min(texts.len()),
                texts.len()
            );
        }
    }

    // Build BM25 index
    eprintln!("Building BM25 keyword index...");
    let bm25 = Bm25Index::build(&texts);

    // Build and write index
    let metadata: Vec<_> = all_chunks.iter().map(|c| c.meta.clone()).collect();
    let index = SearchIndex::new(
        model_id.to_string(),
        embedder.dim(),
        metadata,
        all_embeddings,
        bm25,
    );

    eprintln!("Writing index to {}...", output.display());
    let file = File::create(&output)
        .with_context(|| format!("creating output file {}", output.display()))?;
    index.write_to(BufWriter::new(file))?;

    eprintln!("Done! Index contains {} chunks.", all_chunks.len());
    Ok(())
}

fn cmd_search(
    index_path: PathBuf,
    query: &str,
    top_k: usize,
    model_id: &str,
    mode: SearchMode,
) -> Result<()> {
    // Load index
    eprintln!("Loading index from {}...", index_path.display());
    let file = File::open(&index_path)
        .with_context(|| format!("opening index file {}", index_path.display()))?;
    let index = SearchIndex::read_from(std::io::BufReader::new(file))?;
    eprintln!(
        "  {} chunks, {} dimensions",
        index.metadata.len(),
        index.dim
    );

    match mode {
        SearchMode::Semantic => {
            eprintln!("Loading embedding model: {}...", model_id);
            let embedder = Embedder::new(model_id)?;
            let query_vecs = embedder.embed_batch(&[query])?;
            let results = search(&index, &query_vecs[0], top_k);

            println!("\nSemantic search results for: \"{}\"", query);
            println!("{}", "-".repeat(60));
            for (i, result) in results.iter().enumerate() {
                println!(
                    "{}. [score: {:.4}] {} — {}",
                    i + 1,
                    result.score,
                    result.meta.title,
                    result.meta.url
                );
                if let Some(section) = &result.meta.section {
                    println!("   Section: {}", section);
                }
            }
        }
        SearchMode::Keyword => {
            let results = index.bm25.search(query, top_k);

            println!("\nKeyword (BM25) search results for: \"{}\"", query);
            println!("{}", "-".repeat(60));
            for (i, (chunk_idx, score)) in results.iter().enumerate() {
                let meta = &index.metadata[*chunk_idx];
                println!(
                    "{}. [score: {:.4}] {} — {}",
                    i + 1,
                    score,
                    meta.title,
                    meta.url
                );
                if let Some(section) = &meta.section {
                    println!("   Section: {}", section);
                }
            }
        }
        SearchMode::Hybrid => {
            eprintln!("Loading embedding model: {}...", model_id);
            let embedder = Embedder::new(model_id)?;
            let query_vecs = embedder.embed_batch(&[query])?;

            // Get more candidates than top_k from each method for better fusion
            let fetch_k = top_k * 3;
            let semantic_results = search(&index, &query_vecs[0], fetch_k);
            let bm25_results = index.bm25.search(query, fetch_k);

            let semantic_pairs: Vec<(usize, f32)> = semantic_results
                .iter()
                .map(|r| (r.chunk_index, r.score))
                .collect();

            let hybrid_results = hybrid_rrf(&semantic_pairs, &bm25_results, top_k);

            println!("\nHybrid search results for: \"{}\"", query);
            println!("{}", "-".repeat(60));
            for (i, (chunk_idx, score)) in hybrid_results.iter().enumerate() {
                let meta = &index.metadata[*chunk_idx];
                println!(
                    "{}. [rrf: {:.4}] {} — {}",
                    i + 1,
                    score,
                    meta.title,
                    meta.url
                );
                if let Some(section) = &meta.section {
                    println!("   Section: {}", section);
                }
            }
        }
    }

    Ok(())
}
