// SPDX-License-Identifier: GPL-3.0-only

//! BM25 keyword search index for hybrid retrieval.
//!
//! Complements semantic (embedding) search with exact keyword matching.
//! The BM25 index is built from chunk texts and serialized alongside
//! the embedding index.

use std::collections::HashMap;
use std::io::{Read, Write};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// BM25 parameters.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// A BM25 inverted index built from chunk texts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Index {
    /// Number of documents (chunks).
    pub num_docs: usize,
    /// Average document length in tokens.
    pub avg_doc_len: f64,
    /// Per-document token count.
    pub doc_lengths: Vec<usize>,
    /// Inverted index: term → list of (doc_id, term_frequency).
    pub postings: HashMap<String, Vec<(usize, u32)>>,
}

impl Bm25Index {
    /// Build a BM25 index from chunk texts.
    pub fn build(texts: &[&str]) -> Self {
        let num_docs = texts.len();
        let mut doc_lengths = Vec::with_capacity(num_docs);
        let mut postings: HashMap<String, Vec<(usize, u32)>> = HashMap::new();

        for (doc_id, text) in texts.iter().enumerate() {
            let tokens = tokenize(text);
            doc_lengths.push(tokens.len());

            // Count term frequencies in this document
            let mut tf_map: HashMap<&str, u32> = HashMap::new();
            for token in &tokens {
                *tf_map.entry(token.as_str()).or_default() += 1;
            }

            for (term, freq) in tf_map {
                postings
                    .entry(term.to_string())
                    .or_default()
                    .push((doc_id, freq));
            }
        }

        let total_len: usize = doc_lengths.iter().sum();
        let avg_doc_len = if num_docs > 0 {
            total_len as f64 / num_docs as f64
        } else {
            0.0
        };

        Self {
            num_docs,
            avg_doc_len,
            doc_lengths,
            postings,
        }
    }

    /// Score all documents against a query, returning (doc_id, score) pairs
    /// sorted descending by score.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(usize, f64)> {
        let query_tokens = tokenize(query);
        let mut scores = vec![0.0f64; self.num_docs];

        for token in &query_tokens {
            if let Some(posting_list) = self.postings.get(token.as_str()) {
                let df = posting_list.len() as f64;
                let idf = ((self.num_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();

                for &(doc_id, tf) in posting_list {
                    let tf = tf as f64;
                    let dl = self.doc_lengths[doc_id] as f64;
                    let numerator = tf * (K1 + 1.0);
                    let denominator = tf + K1 * (1.0 - B + B * dl / self.avg_doc_len);
                    scores[doc_id] += idf * numerator / denominator;
                }
            }
        }

        let mut results: Vec<(usize, f64)> = scores
            .into_iter()
            .enumerate()
            .filter(|(_, s)| *s > 0.0)
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// Serialize the BM25 index to a writer (JSON-encoded with length prefix).
    pub fn write_to<W: Write>(&self, mut w: W) -> Result<()> {
        let json = serde_json::to_vec(self).context("serializing BM25 index")?;
        w.write_all(&(json.len() as u32).to_le_bytes())?;
        w.write_all(&json)?;
        Ok(())
    }

    /// Deserialize a BM25 index from a reader.
    pub fn read_from<R: Read>(mut r: R) -> Result<Self> {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf)
            .context("reading BM25 index length")?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut json_buf = vec![0u8; len];
        r.read_exact(&mut json_buf)
            .context("reading BM25 index data")?;

        serde_json::from_slice(&json_buf).context("parsing BM25 index JSON")
    }
}

/// Tokenize text: lowercase, split on non-alphanumeric, filter short tokens.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_string())
        .collect()
}

/// Combine semantic and BM25 results using Reciprocal Rank Fusion (RRF).
///
/// RRF is parameter-free (beyond k) and works well for combining heterogeneous
/// ranking signals. Score: `1/(k + rank_a) + 1/(k + rank_b)`.
pub fn hybrid_rrf(
    semantic_results: &[(usize, f32)],
    bm25_results: &[(usize, f64)],
    top_k: usize,
) -> Vec<(usize, f64)> {
    const RRF_K: f64 = 60.0;

    let mut scores: HashMap<usize, f64> = HashMap::new();

    for (rank, &(doc_id, _)) in semantic_results.iter().enumerate() {
        *scores.entry(doc_id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }

    for (rank, &(doc_id, _)) in bm25_results.iter().enumerate() {
        *scores.entry(doc_id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }

    let mut results: Vec<(usize, f64)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(top_k);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Hello, World! This is a test.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"test".to_string()));
        // Single-char tokens filtered
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[test]
    fn test_bm25_exact_match() {
        let texts = vec![
            "Rust programming language systems",
            "chocolate cake recipe baking",
            "Rust compiler and borrow checker",
        ];
        let index = Bm25Index::build(&texts);
        let results = index.search("Rust programming", 3);

        assert!(!results.is_empty());
        // Doc 0 ("Rust programming") should rank first
        assert_eq!(results[0].0, 0, "expected doc 0 to rank first");
        // Doc 2 also mentions Rust, should appear
        assert!(
            results.iter().any(|(id, _)| *id == 2),
            "expected doc 2 (Rust compiler) to appear"
        );
    }

    #[test]
    fn test_bm25_term_frequency() {
        let texts = vec![
            "rust rust rust is great",        // high tf for "rust"
            "rust is a programming language", // lower tf for "rust"
        ];
        let index = Bm25Index::build(&texts);
        let results = index.search("rust", 2);

        assert_eq!(results.len(), 2);
        // Doc 0 has higher term frequency for "rust"
        assert_eq!(results[0].0, 0);
        assert!(results[0].1 > results[1].1);
    }

    #[test]
    fn test_bm25_no_match() {
        let texts = vec!["rust programming", "python scripting"];
        let index = Bm25Index::build(&texts);
        let results = index.search("javascript", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_bm25_round_trip() {
        let texts = vec!["hello world", "foo bar baz"];
        let index = Bm25Index::build(&texts);

        let mut buf = Vec::new();
        index.write_to(&mut buf).unwrap();

        let restored = Bm25Index::read_from(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(restored.num_docs, 2);
        assert_eq!(restored.doc_lengths, index.doc_lengths);

        // Search should produce same results
        let r1 = index.search("hello", 2);
        let r2 = restored.search("hello", 2);
        assert_eq!(r1.len(), r2.len());
        assert_eq!(r1[0].0, r2[0].0);
    }

    #[test]
    fn test_hybrid_rrf() {
        // Semantic: doc 0 best, doc 1 second
        let semantic = vec![(0, 0.95f32), (1, 0.80), (2, 0.60)];
        // BM25: doc 2 best, doc 0 second
        let bm25 = vec![(2, 5.0f64), (0, 3.0), (1, 1.0)];

        let results = hybrid_rrf(&semantic, &bm25, 3);
        assert_eq!(results.len(), 3);
        // Doc 0 appears rank 1 in semantic and rank 2 in BM25 — should score well
        // Doc 2 appears rank 3 in semantic and rank 1 in BM25 — should also score well
        // Both should beat doc 1 which is middle in both
        let doc0_score = results.iter().find(|(id, _)| *id == 0).unwrap().1;
        let doc1_score = results.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert!(
            doc0_score > doc1_score,
            "doc 0 ({}) should score higher than doc 1 ({})",
            doc0_score,
            doc1_score
        );
    }
}
