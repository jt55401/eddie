// SPDX-License-Identifier: GPL-3.0-only

//! Content chunking: split markdown/HTML into embeddable segments.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// Matches an ATX heading line (`#` through `######`), capturing the marker
/// depth and the heading text. Headings are preserved through parsing (see
/// `crate::parse::strip_markdown`) so this can still find them at chunk time.
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(#{1,6})[ \t]+(.+)$").unwrap());

/// Splits text into rough "sentences", keeping the terminal punctuation.
/// Recognises ASCII `.!?` as well as full-width CJK terminators `。！？`,
/// and does not require trailing whitespace after a full-width terminator.
static SENTENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^.!?。!？\n]+[.!?。!？]*").unwrap());

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
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub granularity: Option<String>,
    pub chunk_index: usize,
}

/// An embeddable text chunk with its metadata.
///
/// `text` is the chunk's own clean content — no overlap prefix baked in, so
/// it is what gets stored, displayed, and BM25-indexed. `overlap` is the
/// tail-words prefix carried over from the previous chunk for embedding
/// context (empty for the first chunk of a document). Use [`Chunk::embed_text`]
/// to get the text that should actually be fed to the embedder.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub overlap: String,
    pub meta: ChunkMeta,
}

impl Chunk {
    /// The text to feed to the embedder: the overlap prefix (if any) followed
    /// by this chunk's own clean text.
    pub fn embed_text(&self) -> String {
        if self.overlap.is_empty() {
            self.text.clone()
        } else {
            format!("{} {}", self.overlap, self.text)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    Heading,
    Semantic,
}

/// Split a document into embeddable chunks.
///
/// Splits on markdown headings first, then breaks oversized sections at
/// paragraph, line, and sentence boundaries, falling back to a hard word
/// window. Uses whitespace/Unicode-word count as a token proxy (conservative:
/// `max_tokens * 0.75`). Overlap repeats the last N words of the previous
/// chunk as a separate prefix (see [`Chunk::embed_text`]) for context
/// continuity.
pub fn chunk_document(doc: &Document, max_tokens: usize, overlap_tokens: usize) -> Vec<Chunk> {
    chunk_document_with_strategy(doc, max_tokens, overlap_tokens, ChunkStrategy::Heading)
}

/// Split a document into embeddable chunks using the requested strategy.
///
/// This is a thin wrapper around [`chunk_document_with_budget`] that measures
/// size with a Unicode-aware word count and applies the traditional
/// `max_tokens * 0.75` conservative token-to-word conversion.
pub fn chunk_document_with_strategy(
    doc: &Document,
    max_tokens: usize,
    overlap_tokens: usize,
    strategy: ChunkStrategy,
) -> Vec<Chunk> {
    let budget = ((max_tokens as f64) * 0.75) as usize;
    chunk_document_with_budget(doc, budget.max(1), overlap_tokens, strategy, &word_count)
}

/// Split a document into embeddable chunks, measuring size with a caller-supplied
/// `count` function (e.g. a real tokenizer's token count) instead of assuming
/// words-as-tokens.
///
/// Guarantees that `Chunk::embed_text()` never exceeds `budget` per `count`:
/// an oversized section is recursively split at paragraph, then line/list-item,
/// then sentence, then hard word-window boundaries; and overlap is shrunk (down
/// to empty) if prepending it would push a chunk over budget.
pub fn chunk_document_with_budget(
    doc: &Document,
    budget: usize,
    overlap: usize,
    strategy: ChunkStrategy,
    count: &dyn Fn(&str) -> usize,
) -> Vec<Chunk> {
    let budget = budget.max(1);
    let body = doc.body.replace("\r\n", "\n");
    let sections = match strategy {
        ChunkStrategy::Heading => split_into_sections(&body),
        ChunkStrategy::Semantic => split_into_semantic_sections(&body, budget, count),
    };

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut chunk_index = 0usize;

    for (heading, text) in &sections {
        // Fold the heading text into the section body (once) so it is
        // searchable in the chunk text, not just in metadata.
        let section_text = match heading {
            Some(h) if !h.is_empty() => {
                if text.trim().is_empty() {
                    h.clone()
                } else {
                    format!("{h}\n\n{text}")
                }
            }
            _ => text.clone(),
        };

        if section_text.trim().is_empty() {
            continue;
        }

        let pieces = split_oversized_budget(&section_text, budget, count);

        for piece in pieces {
            let overlap_str = if chunk_index > 0 && overlap > 0 {
                match chunks.last() {
                    Some(prev) => build_overlap(&prev.text, overlap, &piece, budget, count),
                    None => String::new(),
                }
            } else {
                String::new()
            };

            chunks.push(Chunk {
                text: piece,
                overlap: overlap_str,
                meta: ChunkMeta {
                    title: doc.meta.title.clone(),
                    url: doc.meta.url.clone(),
                    section: heading.clone(),
                    date: doc.meta.date.clone(),
                    granularity: None,
                    chunk_index,
                },
            });
            chunk_index += 1;
        }
    }

    chunks
}

/// Drop chunks whose text (whitespace-normalised, case-insensitive) duplicates
/// an earlier chunk of the same URL, keeping the first occurrence. This
/// removes byte-identical fine/coarse/summary duplicates that a short page
/// otherwise produces (each granularity chunked independently).
pub fn dedupe_chunks(chunks: Vec<Chunk>) -> Vec<Chunk> {
    let mut seen: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();
    let mut result = Vec::with_capacity(chunks.len());

    for chunk in chunks {
        let key = normalize_for_dedupe(&chunk.text);
        let url_seen = seen.entry(chunk.meta.url.clone()).or_default();
        if url_seen.contains(&key) {
            continue;
        }
        url_seen.insert(key);
        result.push(chunk);
    }

    result
}

fn normalize_for_dedupe(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Build a summary chunk from a document's title, frontmatter description,
/// and the list of its heading texts. Returns `None` when the document has
/// neither a description nor any headings (a title alone is not distinctive
/// enough to be worth a dedicated summary chunk).
pub fn summary_chunk(doc: &Document) -> Option<Chunk> {
    let headings: Vec<String> = HEADING_RE
        .captures_iter(&doc.body)
        .map(|c| c[2].trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();

    let description = doc
        .meta
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    if description.is_none() && headings.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let title = doc.meta.title.trim();
    if !title.is_empty() {
        parts.push(title.to_string());
    }
    if let Some(desc) = description {
        parts.push(desc.to_string());
    }
    if !headings.is_empty() {
        parts.push(headings.join(". "));
    }

    let text = parts.join(". ");
    if text.trim().is_empty() {
        return None;
    }

    Some(Chunk {
        text,
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

/// Semantic-ish segmentation using sentence similarity drops and size thresholds.
fn split_into_semantic_sections(
    body: &str,
    target: usize,
    count: &dyn Fn(&str) -> usize,
) -> Vec<(Option<String>, String)> {
    // Semantic mode doesn't track section metadata, so fold heading markers
    // into plain prose rather than leaving literal `#` characters in the text.
    let flattened = HEADING_RE.replace_all(body, "$2");
    let sentences = split_sentences(&flattened);
    if sentences.is_empty() {
        return Vec::new();
    }

    let min_size = (target / 3).max(20);
    let mut sections = Vec::new();
    let mut current = String::new();
    let mut current_size = 0usize;
    let mut prev: Option<&str> = None;

    for sentence in sentences {
        let sentence_size = count(sentence);
        if sentence_size == 0 {
            continue;
        }

        let sim = prev
            .map(|p| sentence_similarity(p, sentence))
            .unwrap_or(1.0);
        let similarity_drop = sim < 0.14;
        let size_break = current_size + sentence_size > target && current_size >= min_size;

        if !current.is_empty() && (size_break || (similarity_drop && current_size >= min_size)) {
            sections.push((None, current.trim().to_string()));
            current.clear();
            current_size = 0;
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence.trim());
        current_size += sentence_size;
        prev = Some(sentence);
    }

    if !current.trim().is_empty() {
        sections.push((None, current.trim().to_string()));
    }

    if sections.is_empty() && !flattened.trim().is_empty() {
        sections.push((None, flattened.trim().to_string()));
    }

    sections
}

/// Split body text into (optional heading, section text) pairs.
/// Splits on lines matching `^#{1,6}\s+`. The heading marker itself is
/// stripped from the section body and captured as the section label; callers
/// that want the heading text to remain searchable fold it back in as plain
/// text (see `chunk_document_with_budget`).
fn split_into_sections(body: &str) -> Vec<(Option<String>, String)> {
    let mut sections = Vec::new();
    let mut last_end = 0;
    let mut current_heading: Option<String> = None;

    for cap in HEADING_RE.captures_iter(body) {
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

/// Split text that exceeds `budget` (per `count`) recursively at paragraph,
/// then line/list-item, then sentence, then hard word-window boundaries.
fn split_oversized_budget(text: &str, budget: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    if count(text) <= budget {
        return vec![text.to_string()];
    }

    let paragraphs = split_nonempty(text, "\n\n");
    if paragraphs.len() > 1 {
        return pack_pieces(&paragraphs, budget, "\n\n", count, split_by_lines);
    }

    split_by_lines(text, budget, count)
}

fn split_by_lines(text: &str, budget: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    if count(text) <= budget {
        return vec![text.to_string()];
    }

    let lines = split_nonempty(text, "\n");
    if lines.len() > 1 {
        return pack_pieces(&lines, budget, "\n", count, split_by_sentences);
    }

    split_by_sentences(text, budget, count)
}

fn split_by_sentences(text: &str, budget: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    if count(text) <= budget {
        return vec![text.to_string()];
    }

    let sentences = split_sentences(text);
    if sentences.len() > 1 {
        return pack_pieces(&sentences, budget, " ", count, split_by_words);
    }

    split_by_words(text, budget, count)
}

/// Last resort: pack Unicode word-boundary tokens (which include the
/// original inter-word text, so no artificial separators are introduced)
/// until adding the next one would exceed budget.
fn split_by_words(text: &str, budget: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    if count(text) <= budget {
        return vec![text.to_string()];
    }

    let tokens: Vec<&str> = text.split_word_bounds().collect();
    if tokens.len() <= 1 {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();
    let mut current = String::new();

    for tok in tokens {
        let candidate = format!("{current}{tok}");
        if !current.is_empty() && count(&candidate) > budget {
            result.push(std::mem::take(&mut current));
            current = tok.to_string();
        } else {
            current = candidate;
        }
    }

    if !current.trim().is_empty() {
        result.push(current);
    }

    if result.is_empty() {
        vec![text.to_string()]
    } else {
        result
    }
}

/// A recursive splitter for a single oversized piece (paragraph, line,
/// sentence, ...), used by [`pack_pieces`] as its next-level fallback.
type SplitFn = fn(&str, usize, &dyn Fn(&str) -> usize) -> Vec<String>;

/// Greedily pack `pieces` into chunks that fit `budget`, recursively
/// splitting any single piece that alone exceeds budget via `split_piece`.
fn pack_pieces(
    pieces: &[&str],
    budget: usize,
    separator: &str,
    count: &dyn Fn(&str) -> usize,
    split_piece: SplitFn,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for piece in pieces {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }

        if count(piece) > budget {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            result.extend(split_piece(piece, budget, count));
            continue;
        }

        if current.is_empty() {
            current = piece.to_string();
        } else {
            let candidate = format!("{current}{separator}{piece}");
            if count(&candidate) <= budget {
                current = candidate;
            } else {
                result.push(std::mem::take(&mut current));
                current = piece.to_string();
            }
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

fn split_nonempty<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
    text.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Build the overlap prefix for a chunk from the previous chunk's clean text,
/// shrinking the requested word count (down to empty) until the combination
/// of overlap + this chunk's piece fits `budget` per `count`.
fn build_overlap(
    prev_text: &str,
    requested: usize,
    next_piece: &str,
    budget: usize,
    count: &dyn Fn(&str) -> usize,
) -> String {
    let mut n = requested;
    loop {
        if n == 0 {
            return String::new();
        }
        let candidate = tail_words(prev_text, n);
        if candidate.is_empty() {
            return String::new();
        }
        let combined = format!("{candidate} {next_piece}");
        if count(&combined) <= budget {
            return candidate;
        }
        n -= 1;
    }
}

fn split_sentences(text: &str) -> Vec<&str> {
    SENTENCE_RE
        .find_iter(text)
        .map(|m| m.as_str().trim())
        .filter(|s| !s.is_empty())
        .collect()
}

fn sentence_similarity(a: &str, b: &str) -> f32 {
    let ta = token_set(a);
    let tb = token_set(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count() as f32;
    let union = ta.union(&tb).count() as f32;
    if union == 0.0 {
        return 0.0;
    }
    inter / union
}

fn token_set(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.len() > 2)
        .collect()
}

/// Count Unicode "words" in text. Uses word-boundary segmentation (UAX #29)
/// rather than whitespace splitting, so unspaced scripts (CJK, Thai, etc.)
/// are counted proportionally to their real size (each CJK ideograph counts
/// as its own word) instead of collapsing to a single "word".
fn word_count(text: &str) -> usize {
    text.unicode_words().count()
}

/// Get the last N Unicode words from text as a string, preserving the
/// original substring (no re-joining with synthetic separators, so scripts
/// without whitespace between words are not corrupted).
fn tail_words(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let indices: Vec<usize> = text.unicode_word_indices().map(|(i, _)| i).collect();
    if indices.is_empty() {
        return String::new();
    }
    if indices.len() <= n {
        return text.trim().to_string();
    }
    let start = indices[indices.len() - n];
    text[start..].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{ContentParser, HugoParser};
    use std::path::Path;

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
        assert!(
            chunks.len() >= 3,
            "expected at least 3 chunks, got {}",
            chunks.len()
        );
        assert!(chunks[0].meta.section.is_none());
        assert_eq!(chunks[1].meta.section.as_deref(), Some("Section One"));
        assert_eq!(chunks[2].meta.section.as_deref(), Some("Section Two"));
    }

    #[test]
    fn test_large_section_splits() {
        // Create a section with ~200 words (will exceed 256 * 0.75 = 192 effective limit)
        let words: String = (0..200)
            .map(|i| format!("word{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let body = format!("## Big Section\n\n{}\n\n{}", words, words);
        let doc = make_doc(&body);
        let chunks = chunk_document(&doc, 256, 0);
        assert!(
            chunks.len() >= 2,
            "expected at least 2 chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn test_metadata_preserved() {
        let doc = make_doc("Some content.");
        let chunks = chunk_document(&doc, 256, 0);
        assert_eq!(chunks[0].meta.title, "Test");
        assert_eq!(chunks[0].meta.url, "/test/");
        assert_eq!(chunks[0].meta.date.as_deref(), Some("2024-01-01"));
        assert_eq!(chunks[0].meta.chunk_index, 0);
    }

    #[test]
    fn test_overlap() {
        let body = "## Part One\n\nFirst section with several words here.\n\n## Part Two\n\nSecond section content.";
        let doc = make_doc(body);
        let chunks = chunk_document(&doc, 256, 3);
        assert!(chunks.len() >= 2);
        assert!(
            chunks[0].overlap.is_empty(),
            "first chunk should have no overlap"
        );
        if chunks.len() >= 2 {
            let first_words: Vec<&str> = chunks[0].text.split_whitespace().collect();
            let last_3 = first_words[first_words.len().saturating_sub(3)..].join(" ");
            assert_eq!(
                chunks[1].overlap, last_3,
                "overlap should carry the previous chunk's tail words, stored separately from text"
            );
            assert!(
                chunks[1].embed_text().starts_with(&chunks[1].overlap),
                "embed_text should prepend the overlap"
            );
            assert!(
                chunks[1].embed_text().ends_with(&chunks[1].text),
                "embed_text should end with the chunk's own clean text"
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
    fn test_word_count_cjk() {
        // Unspaced scripts must count roughly proportionally to their size,
        // not collapse to a single "word".
        assert!(
            word_count("这是一个测试句子。") > 1,
            "CJK text should not count as a single word"
        );
    }

    #[test]
    fn test_tail_words() {
        assert_eq!(tail_words("a b c d e", 3), "c d e");
        assert_eq!(tail_words("short", 5), "short");
    }

    #[test]
    fn test_semantic_strategy_produces_chunks() {
        let body = "The subject built search systems for years. They worked at multiple companies. Rust remains a core tool. Ski conditions were rough. Focus moved to retrieval quality and grounding.";
        let doc = make_doc(body);
        let chunks = chunk_document_with_strategy(&doc, 64, 0, ChunkStrategy::Semantic);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.meta.section.is_none()));
    }

    #[test]
    fn parsed_hugo_document_preserves_heading_sections_for_chunking() {
        // Regression test for the critical bug where strip_markdown deleted
        // `#` heading markers before the chunker ever saw them, so the
        // Heading strategy never found a section boundary. This runs a real
        // CMS parser (which used to eat the headings) followed by chunking.
        let parser = HugoParser;
        let content = "---\ntitle: Guide\n---\n\nIntro paragraph.\n\n## Section One\n\nFirst section body.\n\n## Section Two\n\nSecond section body.\n";
        let (meta, body) = parser
            .parse_file(content, Path::new("content/guide.md"), Path::new("content"))
            .unwrap()
            .unwrap();
        let doc = Document {
            meta,
            body,
            source_path: "content/guide.md".to_string(),
        };
        let chunks = chunk_document_with_strategy(&doc, 256, 0, ChunkStrategy::Heading);

        assert!(
            chunks.len() >= 3,
            "expected at least 3 chunks, got {}",
            chunks.len()
        );
        assert_eq!(chunks[0].meta.section, None);
        assert_eq!(chunks[1].meta.section.as_deref(), Some("Section One"));
        assert_eq!(chunks[2].meta.section.as_deref(), Some("Section Two"));
        assert!(
            chunks[1].text.contains("Section One"),
            "heading text should be folded into the chunk body so it is searchable: {:?}",
            chunks[1].text
        );
        assert!(chunks[1].text.contains("First section body"));
    }

    #[test]
    fn oversized_single_paragraph_is_split_recursively() {
        // A single oversized paragraph (no blank-line breaks at all) must
        // still be split — previously split_oversized only recursed when
        // there were 2+ paragraphs.
        let long_paragraph: String = (0..900)
            .map(|i| format!("word{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let body = format!("Short para.\n\n{}\n\nAnother.", long_paragraph);
        let doc = make_doc(&body);
        let chunks = chunk_document(&doc, 256, 0); // effective budget = 192 words
        for c in &chunks {
            assert!(
                word_count(&c.text) <= 192,
                "chunk exceeded budget: {} words",
                word_count(&c.text)
            );
        }
        assert!(
            chunks.len() >= 4,
            "900-word paragraph at a ~192-word budget should require several chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn bullet_list_joined_by_single_newlines_is_split() {
        let items: String = (0..300)
            .map(|i| format!("- item number {i} with a bit of extra text"))
            .collect::<Vec<_>>()
            .join("\n");
        let doc = make_doc(&items);
        let chunks = chunk_document(&doc, 256, 0);
        for c in &chunks {
            assert!(
                word_count(&c.text) <= 192,
                "chunk exceeded budget: {} words",
                word_count(&c.text)
            );
        }
        assert!(chunks.len() > 1);
    }

    #[test]
    fn crlf_paragraphs_are_normalized_before_splitting() {
        let para: String = (0..300)
            .map(|i| format!("w{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let body = format!("{}\r\n\r\n{}\r\n\r\n{}", para, para, para);
        let doc = make_doc(&body);
        let chunks = chunk_document(&doc, 256, 0);
        assert!(
            chunks.len() >= 3,
            "CRLF-separated paragraphs should still be recognized as separate pieces, got {} chunks",
            chunks.len()
        );
        for c in &chunks {
            assert!(
                word_count(&c.text) <= 192,
                "chunk exceeded budget: {} words",
                word_count(&c.text)
            );
        }
    }

    #[test]
    fn japanese_text_splits_into_multiple_chunks_under_small_budget() {
        let sentence =
            "これは日本語のテスト文章です。長い文章を小さなチャンクに分割できるか確認します。";
        let body: String = std::iter::repeat_n(sentence, 15)
            .collect::<Vec<_>>()
            .join("");
        let doc = make_doc(&body);
        let chunks = chunk_document(&doc, 64, 0);
        assert!(
            chunks.len() > 1,
            "expected the Japanese document to split into multiple chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn dedupe_chunks_drops_same_url_duplicates_but_keeps_first() {
        let doc = make_doc("Some content.");
        let mut fine = chunk_document(&doc, 256, 0);
        let mut coarse = chunk_document(&doc, 512, 0);
        fine[0].meta.granularity = Some("fine".to_string());
        coarse[0].meta.granularity = Some("coarse".to_string());

        let combined = vec![fine[0].clone(), coarse[0].clone()];
        let deduped = dedupe_chunks(combined);
        assert_eq!(
            deduped.len(),
            1,
            "identical text for the same URL should collapse to one chunk"
        );
        assert_eq!(deduped[0].meta.granularity.as_deref(), Some("fine"));
    }

    #[test]
    fn dedupe_chunks_keeps_distinct_urls() {
        let mut a = make_doc("Some content.");
        a.meta.url = "/a/".to_string();
        let mut b = make_doc("Some content.");
        b.meta.url = "/b/".to_string();

        let chunks = vec![
            chunk_document(&a, 256, 0).remove(0),
            chunk_document(&b, 256, 0).remove(0),
        ];
        let deduped = dedupe_chunks(chunks);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn summary_chunk_uses_description_and_headings() {
        let mut doc = make_doc("## Section One\n\nBody one.\n\n## Section Two\n\nBody two.");
        doc.meta.description = Some("A helpful guide.".to_string());
        let summary = summary_chunk(&doc).expect("expected a summary chunk");
        assert!(summary.text.contains("Test"));
        assert!(summary.text.contains("A helpful guide."));
        assert!(summary.text.contains("Section One"));
        assert!(summary.text.contains("Section Two"));
        assert_eq!(summary.meta.granularity.as_deref(), Some("summary"));
    }

    #[test]
    fn summary_chunk_none_without_description_or_headings() {
        let doc = make_doc("Just a plain paragraph with no headings.");
        assert!(summary_chunk(&doc).is_none());
    }
}
