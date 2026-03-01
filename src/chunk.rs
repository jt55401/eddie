// SPDX-License-Identifier: GPL-3.0-only

//! Content chunking: split markdown/HTML into embeddable segments.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Metadata extracted from a document's frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub date: Option<String>,
}

/// A parsed document with frontmatter metadata and body content.
#[derive(Debug, Clone)]
pub struct Document {
    pub meta: DocumentMeta,
    pub body: String,
    pub source_path: String,
}

/// Metadata attached to each chunk, linking it back to its source document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub title: String,
    pub url: String,
    pub section: Option<String>,
    pub chunk_index: usize,
}

/// An embeddable text chunk with its metadata.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub meta: ChunkMeta,
}

/// Split a document into embeddable chunks.
///
/// Splits on markdown headings first, then breaks oversized sections at paragraph
/// and sentence boundaries. Uses whitespace-word count as a token proxy
/// (conservative: `max_tokens * 0.75`). Overlap repeats the last N words at the
/// start of the next chunk for context continuity.
pub fn chunk_document(doc: &Document, max_tokens: usize, overlap_tokens: usize) -> Vec<Chunk> {
    let effective_max = (max_tokens as f64 * 0.75) as usize;
    let sections = split_into_sections(&doc.body);

    let mut chunks = Vec::new();
    let mut chunk_index = 0;

    for (heading, text) in &sections {
        let pieces = split_oversized(text, effective_max);

        for piece in pieces {
            let text = if chunk_index > 0 && overlap_tokens > 0 {
                // Prepend overlap from previous chunk
                if let Some(prev) = chunks.last() {
                    let prev_chunk: &Chunk = prev;
                    let overlap = tail_words(&prev_chunk.text, overlap_tokens);
                    if overlap.is_empty() {
                        piece.clone()
                    } else {
                        format!("{} {}", overlap, piece)
                    }
                } else {
                    piece.clone()
                }
            } else {
                piece.clone()
            };

            chunks.push(Chunk {
                text,
                meta: ChunkMeta {
                    title: doc.meta.title.clone(),
                    url: doc.meta.url.clone(),
                    section: heading.clone(),
                    chunk_index,
                },
            });
            chunk_index += 1;
        }
    }

    chunks
}

/// Split body text into (optional heading, section text) pairs.
/// Splits on lines matching `^#{1,6}\s+`.
fn split_into_sections(body: &str) -> Vec<(Option<String>, String)> {
    let heading_re = Regex::new(r"(?m)^(#{1,6})\s+(.+)$").unwrap();
    let mut sections = Vec::new();
    let mut last_end = 0;
    let mut current_heading: Option<String> = None;

    for cap in heading_re.captures_iter(body) {
        let m = cap.get(0).unwrap();
        let start = m.start();

        // Collect text before this heading
        if start > last_end {
            let text = body[last_end..start].trim().to_string();
            if !text.is_empty() {
                sections.push((current_heading.clone(), text));
            }
        }

        current_heading = Some(cap[2].trim().to_string());
        last_end = m.end();
    }

    // Remaining text after the last heading
    let remaining = body[last_end..].trim().to_string();
    if !remaining.is_empty() {
        sections.push((current_heading, remaining));
    }

    // If body had no headings and no sections were created, return one section
    if sections.is_empty() && !body.trim().is_empty() {
        sections.push((None, body.trim().to_string()));
    }

    sections
}

/// Split text that exceeds the word limit, first at paragraph boundaries,
/// then at sentence boundaries.
fn split_oversized(text: &str, max_words: usize) -> Vec<String> {
    if word_count(text) <= max_words {
        return vec![text.to_string()];
    }

    // Try paragraph splits first
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    if paragraphs.len() > 1 {
        return merge_pieces(&paragraphs, max_words, "\n\n");
    }

    // Fall back to sentence splits
    let sentence_re = Regex::new(r"[.!?]+\s+").unwrap();
    let sentences: Vec<&str> = sentence_re.split(text).collect();
    if sentences.len() > 1 {
        return merge_pieces(&sentences, max_words, ". ");
    }

    // Can't split further — return as-is
    vec![text.to_string()]
}

/// Merge pieces into chunks that don't exceed max_words.
fn merge_pieces(pieces: &[&str], max_words: usize, separator: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for piece in pieces {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }

        if current.is_empty() {
            current = piece.to_string();
        } else if word_count(&current) + word_count(piece) <= max_words {
            current.push_str(separator);
            current.push_str(piece);
        } else {
            if !current.is_empty() {
                result.push(current);
            }
            current = piece.to_string();
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

/// Count whitespace-delimited words.
fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Get the last N words from text as a string.
fn tail_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= n {
        return text.to_string();
    }
    words[words.len() - n..].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_doc(body: &str) -> Document {
        Document {
            meta: DocumentMeta {
                title: "Test".to_string(),
                url: "/test/".to_string(),
                description: None,
                tags: vec!["rust".to_string()],
                date: Some("2024-01-01".to_string()),
            },
            body: body.to_string(),
            source_path: "test.md".to_string(),
        }
    }

    #[test]
    fn test_short_document_single_chunk() {
        let doc = make_doc("A short document with just a few words.");
        let chunks = chunk_document(&doc, 256, 0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("short document"));
    }

    #[test]
    fn test_section_split() {
        let body = "Intro text.\n\n## Section One\n\nFirst section body.\n\n## Section Two\n\nSecond section body.";
        let doc = make_doc(body);
        let chunks = chunk_document(&doc, 256, 0);
        assert!(chunks.len() >= 3, "expected at least 3 chunks, got {}", chunks.len());
        assert!(chunks[0].meta.section.is_none());
        assert_eq!(chunks[1].meta.section.as_deref(), Some("Section One"));
        assert_eq!(chunks[2].meta.section.as_deref(), Some("Section Two"));
    }

    #[test]
    fn test_large_section_splits() {
        // Create a section with ~200 words (will exceed 256 * 0.75 = 192 effective limit)
        let words: String = (0..200).map(|i| format!("word{}", i)).collect::<Vec<_>>().join(" ");
        let body = format!("## Big Section\n\n{}\n\n{}", words, words);
        let doc = make_doc(&body);
        let chunks = chunk_document(&doc, 256, 0);
        assert!(chunks.len() >= 2, "expected at least 2 chunks, got {}", chunks.len());
    }

    #[test]
    fn test_metadata_preserved() {
        let doc = make_doc("Some content.");
        let chunks = chunk_document(&doc, 256, 0);
        assert_eq!(chunks[0].meta.title, "Test");
        assert_eq!(chunks[0].meta.url, "/test/");
        assert_eq!(chunks[0].meta.chunk_index, 0);
    }

    #[test]
    fn test_overlap() {
        let body = "## Part One\n\nFirst section with several words here.\n\n## Part Two\n\nSecond section content.";
        let doc = make_doc(body);
        let chunks = chunk_document(&doc, 256, 3);
        assert!(chunks.len() >= 2);
        if chunks.len() >= 2 {
            // The second chunk should start with overlap words from the first
            let first_words: Vec<&str> = chunks[0].text.split_whitespace().collect();
            let last_3 = first_words[first_words.len().saturating_sub(3)..].join(" ");
            assert!(
                chunks[1].text.starts_with(&last_3),
                "expected chunk 1 to start with '{}', got '{}'",
                last_3,
                &chunks[1].text[..chunks[1].text.len().min(50)]
            );
        }
    }

    #[test]
    fn test_word_count() {
        assert_eq!(word_count("hello world"), 2);
        assert_eq!(word_count("  spaced  out  "), 2);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn test_tail_words() {
        assert_eq!(tail_words("a b c d e", 3), "c d e");
        assert_eq!(tail_words("short", 5), "short");
    }
}
