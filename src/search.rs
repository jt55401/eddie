// SPDX-License-Identifier: GPL-3.0-only

//! Search: embed a query and find the top-k nearest chunks by cosine similarity.

use crate::chunk::ChunkMeta;
use crate::index::SearchIndex;

/// A single search result with its score and metadata.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub score: f32,
    pub chunk_index: usize,
    pub meta: ChunkMeta,
}

/// Find the top-k chunks most similar to the query embedding.
/// Assumes embeddings are L2-normalized, so dot product equals cosine similarity.
pub fn search(index: &SearchIndex, query_embedding: &[f32], top_k: usize) -> Vec<SearchResult> {
    let mut scored: Vec<SearchResult> = index
        .metadata
        .iter()
        .enumerate()
        .map(|(i, meta)| {
            let emb = index.embedding(i);
            SearchResult {
                score: dot(query_embedding, emb),
                chunk_index: i,
                meta: meta.clone(),
            }
        })
        .collect();

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    scored
}

/// Dot product of two vectors.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bm25::Bm25Index;

    fn make_index(embeddings: Vec<Vec<f32>>) -> SearchIndex {
        let dim = embeddings[0].len();
        let n = embeddings.len();
        let metadata: Vec<ChunkMeta> = embeddings
            .iter()
            .enumerate()
            .map(|(i, _)| ChunkMeta {
                title: format!("Doc {}", i),
                url: format!("/doc-{}/", i),
                section: None,
                chunk_index: i,
            })
            .collect();
        let flat: Vec<f32> = embeddings.into_iter().flatten().collect();
        let dummy_texts: Vec<String> = (0..n).map(|i| format!("doc {}", i)).collect();
        let text_refs: Vec<&str> = dummy_texts.iter().map(|s| s.as_str()).collect();
        let bm25 = Bm25Index::build(&text_refs);
        SearchIndex::new("test".to_string(), dim, metadata, flat, bm25, dummy_texts)
    }

    #[test]
    fn test_cosine_identical() {
        let v = vec![0.5, 0.5, 0.5, 0.5];
        assert!((dot(&v, &v) - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_cosine_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((dot(&a, &b)).abs() < 0.001);
    }

    #[test]
    fn test_search_top_k_ordering() {
        // Three 2D normalized vectors
        let index = make_index(vec![
            vec![1.0, 0.0],  // doc 0: points right
            vec![0.0, 1.0],  // doc 1: points up
            vec![0.707, 0.707], // doc 2: 45 degrees
        ]);

        // Query points right — doc 0 should be best match, doc 2 second
        let query = vec![1.0, 0.0];
        let results = search(&index, &query, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].chunk_index, 0);
        assert_eq!(results[1].chunk_index, 2);
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn test_search_top_k_truncation() {
        let index = make_index(vec![
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![0.707, 0.707],
        ]);
        let query = vec![1.0, 0.0];
        let results = search(&index, &query, 1);
        assert_eq!(results.len(), 1);
    }
}
