// SPDX-License-Identifier: GPL-3.0-only

//! BM25 keyword arm for hybrid retrieval.
//!
//! The index is built from the clean chunk texts and stored in the `bm25`
//! section of the `.ed` payload as a sorted term dictionary with binary
//! postings, so identical content always produces identical bytes.
//!
//! Section body:
//!
//! ```text
//! u32 num_docs | f32 avg_len | u32 doc_lengths[num_docs] | u32 terms
//! per term: u16 len | UTF-8 bytes | u32 postings | (varint doc_delta, u32 tf)*
//! ```
//!
//! Terms are sorted bytewise; postings are sorted by document id and stored
//! as deltas (the first delta is the absolute id, later deltas are >= 1).

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::manifest::Bm25Params;

/// A BM25 inverted index built from chunk texts.
#[derive(Debug, Clone, PartialEq)]
pub struct Bm25Index {
    /// Number of documents (chunks).
    pub num_docs: usize,
    /// Average document length in tokens.
    pub avg_doc_len: f64,
    /// Per-document token count.
    pub doc_lengths: Vec<u32>,
    /// Sorted term dictionary; `postings[i]` belongs to `terms[i]`.
    pub terms: Vec<String>,
    /// Per term: `(doc_id, term_frequency)` sorted by `doc_id`.
    pub postings: Vec<Vec<(u32, u32)>>,
    pub params: Bm25Params,
}

impl Bm25Index {
    /// Build a BM25 index from chunk texts with the default parameters.
    pub fn build(texts: &[&str]) -> Self {
        Self::build_with_params(texts, Bm25Params::default())
    }

    /// Build a BM25 index from chunk texts.
    pub fn build_with_params(texts: &[&str], params: Bm25Params) -> Self {
        let num_docs = texts.len();
        let mut doc_lengths = Vec::with_capacity(num_docs);
        let mut postings_map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();

        for (doc_id, text) in texts.iter().enumerate() {
            let tokens = tokenize(text);
            doc_lengths.push(tokens.len() as u32);

            let mut tf_map: HashMap<&str, u32> = HashMap::new();
            for token in &tokens {
                *tf_map.entry(token.as_str()).or_default() += 1;
            }

            // Documents are visited in order, so every posting list stays
            // sorted by doc id without an explicit sort.
            for (term, freq) in tf_map {
                postings_map
                    .entry(term.to_string())
                    .or_default()
                    .push((doc_id as u32, freq));
            }
        }

        let mut terms: Vec<String> = postings_map.keys().cloned().collect();
        terms.sort_unstable();
        let postings: Vec<Vec<(u32, u32)>> = terms
            .iter()
            .map(|t| postings_map.remove(t).unwrap_or_default())
            .collect();

        let total_len: u64 = doc_lengths.iter().map(|&l| l as u64).sum();
        let avg_doc_len = if num_docs > 0 {
            total_len as f64 / num_docs as f64
        } else {
            0.0
        };

        Self {
            num_docs,
            avg_doc_len,
            doc_lengths,
            terms,
            postings,
            params,
        }
    }

    /// Number of distinct terms.
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Posting list for a term, if present.
    pub fn postings_for(&self, term: &str) -> Option<&[(u32, u32)]> {
        self.terms
            .binary_search_by(|t| t.as_str().cmp(term))
            .ok()
            .map(|i| self.postings[i].as_slice())
    }

    /// Score all documents against a query, returning `(doc_id, score)` pairs
    /// sorted by score descending, ties broken by ascending doc id.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(usize, f64)> {
        if top_k == 0 || self.num_docs == 0 {
            return Vec::new();
        }
        let mut query_tokens = tokenize(query);
        query_tokens.sort_unstable();
        query_tokens.dedup();

        let k1 = self.params.k1;
        let b = self.params.b;
        let avg = if self.avg_doc_len > 0.0 {
            self.avg_doc_len
        } else {
            1.0
        };
        let n = self.num_docs as f64;

        let mut scores: HashMap<u32, f64> = HashMap::new();
        for token in &query_tokens {
            let Some(posting_list) = self.postings_for(token) else {
                continue;
            };
            let df = posting_list.len() as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

            for &(doc_id, tf) in posting_list {
                let tf = tf as f64;
                let dl = self.doc_lengths[doc_id as usize] as f64;
                let numerator = tf * (k1 + 1.0);
                let denominator = tf + k1 * (1.0 - b + b * dl / avg);
                *scores.entry(doc_id).or_default() += idf * numerator / denominator;
            }
        }

        let mut results: Vec<(usize, f64)> = scores
            .into_iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(d, s)| (d as usize, s))
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        results.truncate(top_k);
        results
    }

    /// Serialize the section body.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(self.num_docs)?.to_le_bytes());
        out.extend_from_slice(&(self.avg_doc_len as f32).to_le_bytes());
        for &len in &self.doc_lengths {
            out.extend_from_slice(&len.to_le_bytes());
        }
        out.extend_from_slice(&u32::try_from(self.terms.len())?.to_le_bytes());
        for (term, postings) in self.terms.iter().zip(&self.postings) {
            let bytes = term.as_bytes();
            let len = u16::try_from(bytes.len())
                .with_context(|| format!("bm25 term longer than 65535 bytes: {:?}", term))?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(&u32::try_from(postings.len())?.to_le_bytes());
            let mut prev = 0u32;
            for (i, &(doc, tf)) in postings.iter().enumerate() {
                let delta = if i == 0 { doc } else { doc - prev };
                write_varint(&mut out, delta);
                out.extend_from_slice(&tf.to_le_bytes());
                prev = doc;
            }
        }
        Ok(out)
    }

    /// Parse a section body. `expected_docs` is the chunk count from the
    /// manifest; every document id is validated against it.
    pub fn from_bytes(body: &[u8], expected_docs: usize, params: Bm25Params) -> Result<Self> {
        let mut c = ByteCursor::new(body);
        let num_docs = c.u32().context("bm25 num_docs")? as usize;
        if num_docs != expected_docs {
            bail!(
                "bm25 num_docs {} does not match chunk count {}",
                num_docs,
                expected_docs
            );
        }
        let avg_doc_len = c.f32().context("bm25 avg_len")? as f64;
        if !avg_doc_len.is_finite() || avg_doc_len < 0.0 {
            bail!("bm25 avg_len is not a finite non-negative number");
        }
        let mut doc_lengths = Vec::with_capacity(num_docs.min(c.remaining() / 4));
        for _ in 0..num_docs {
            doc_lengths.push(c.u32().context("bm25 doc_lengths")?);
        }
        let term_count = c.u32().context("bm25 term count")? as usize;
        let mut terms: Vec<String> = Vec::with_capacity(term_count.min(c.remaining() / 7));
        let mut postings: Vec<Vec<(u32, u32)>> = Vec::with_capacity(terms.capacity());
        for i in 0..term_count {
            let len = c.u16().with_context(|| format!("bm25 term {} length", i))? as usize;
            let bytes = c.bytes(len).with_context(|| format!("bm25 term {}", i))?;
            let term = std::str::from_utf8(bytes)
                .with_context(|| format!("bm25 term {} is not UTF-8", i))?
                .to_string();
            if let Some(prev) = terms.last()
                && prev.as_str() >= term.as_str()
            {
                bail!("bm25 term dictionary is not strictly sorted at {:?}", term);
            }
            let count = c
                .u32()
                .with_context(|| format!("bm25 postings count for {:?}", term))?
                as usize;
            if count == 0 {
                bail!("bm25 term {:?} has no postings", term);
            }
            let mut list = Vec::with_capacity(count.min(c.remaining() / 5));
            let mut prev = 0u32;
            for j in 0..count {
                let delta = c
                    .varint()
                    .with_context(|| format!("bm25 posting {} of {:?}", j, term))?;
                let doc = if j == 0 {
                    delta
                } else {
                    if delta == 0 {
                        bail!("bm25 postings for {:?} are not strictly increasing", term);
                    }
                    prev.checked_add(delta).context("bm25 doc id overflow")?
                };
                if doc as usize >= num_docs {
                    bail!(
                        "bm25 posting doc id {} out of range (num_docs {})",
                        doc,
                        num_docs
                    );
                }
                let tf = c.u32().context("bm25 tf")?;
                if tf == 0 {
                    bail!("bm25 posting with zero tf for {:?}", term);
                }
                list.push((doc, tf));
                prev = doc;
            }
            terms.push(term);
            postings.push(list);
        }
        if c.remaining() != 0 {
            bail!("bm25 section has {} trailing bytes", c.remaining());
        }
        Ok(Self {
            num_docs,
            avg_doc_len,
            doc_lengths,
            terms,
            postings,
            params,
        })
    }
}

/// Tokenize text for BM25 and query-term matching.
///
/// Lowercase; runs of Unicode alphanumerics become one token each; runs of
/// CJK characters (Han, Hiragana, Katakana, Hangul) become character bigrams
/// (a lone CJK character is emitted as is). Single-character Latin tokens are
/// kept so `C`, `R` and `Go` remain searchable.
pub fn tokenize(text: &str) -> Vec<String> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Other,
        Word,
        Cjk,
    }
    fn class(ch: char) -> Class {
        if is_cjk(ch) {
            Class::Cjk
        } else if ch.is_alphanumeric() {
            Class::Word
        } else {
            Class::Other
        }
    }

    let mut out = Vec::new();
    let mut run = String::new();
    let mut run_class = Class::Other;

    let flush = |run: &mut String, class: Class, out: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        match class {
            Class::Word => out.push(run.to_lowercase()),
            Class::Cjk => {
                let chars: Vec<char> = run.chars().collect();
                if chars.len() == 1 {
                    out.push(run.clone());
                } else {
                    for pair in chars.windows(2) {
                        out.push(pair.iter().collect());
                    }
                }
            }
            Class::Other => {}
        }
        run.clear();
    };

    for ch in text.chars() {
        let c = class(ch);
        if c != run_class {
            flush(&mut run, run_class, &mut out);
            run_class = c;
        }
        if c != Class::Other {
            run.push(ch);
        }
    }
    flush(&mut run, run_class, &mut out);
    out
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF      // Hiragana, Katakana
        | 0x3400..=0x4DBF    // CJK Extension A
        | 0x4E00..=0x9FFF    // CJK Unified Ideographs
        | 0xF900..=0xFAFF    // CJK Compatibility Ideographs
        | 0x1100..=0x11FF    // Hangul Jamo
        | 0x3130..=0x318F    // Hangul Compatibility Jamo
        | 0xAC00..=0xD7AF    // Hangul Syllables
        | 0x20000..=0x2A6DF  // CJK Extension B
        | 0x2A700..=0x2EBEF  // CJK Extensions C-F
        | 0x30000..=0x3134F // CJK Extension G
    )
}

/// LEB128 unsigned varint.
pub(crate) fn write_varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Bounds-checked little-endian reader over a byte slice. Every read checks
/// the remaining length before touching the data, so a truncated or hostile
/// buffer surfaces as an error rather than a panic or an oversized allocation.
pub(crate) struct ByteCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteCursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            bail!(
                "need {} bytes at offset {} but only {} remain",
                n,
                self.pos,
                self.remaining()
            );
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub(crate) fn varint(&mut self) -> Result<u32> {
        let mut result: u32 = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            let payload = (byte & 0x7F) as u32;
            if shift == 28 && payload > 0x0F {
                bail!("varint overflows u32");
            }
            result |= payload << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        bail!("varint longer than 5 bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_basic_and_single_chars() {
        let tokens = tokenize("Hello, World! This is a test of C++ and R.");
        assert_eq!(
            tokens,
            vec![
                "hello", "world", "this", "is", "a", "test", "of", "c", "and", "r"
            ]
        );
    }

    #[test]
    fn tokenize_cjk_bigrams_and_mixed_scripts() {
        assert_eq!(
            tokenize("我住在东京。"),
            vec!["我住", "住在", "在东", "东京"]
        );
        assert_eq!(tokenize("东京"), vec!["东京"]);
        assert_eq!(tokenize("京"), vec!["京"]);
        assert_eq!(
            tokenize("东京tokyo 1.93.1"),
            vec!["东京", "tokyo", "1", "93", "1"]
        );
        assert_eq!(tokenize("Größe résumé"), vec!["größe", "résumé"]);
    }

    #[test]
    fn tokenize_query_matches_document_bigrams() {
        let index = Bm25Index::build(&["我住在东京。", "hello world"]);
        let results = index.search("东京", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
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
        assert_eq!(results[0].0, 0);
        assert!(results.iter().any(|(id, _)| *id == 2));
    }

    #[test]
    fn test_bm25_term_frequency() {
        let texts = vec!["rust rust rust is great", "rust is a programming language"];
        let index = Bm25Index::build(&texts);
        let results = index.search("rust", 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 0);
        assert!(results[0].1 > results[1].1);
    }

    #[test]
    fn test_bm25_no_match_and_empty_query() {
        let index = Bm25Index::build(&["rust programming", "python scripting"]);
        assert!(index.search("javascript", 5).is_empty());
        assert!(index.search("??", 5).is_empty());
        assert!(index.search("rust", 0).is_empty());
    }

    #[test]
    fn ties_break_by_doc_id() {
        let index = Bm25Index::build(&["alpha beta", "alpha beta", "alpha beta"]);
        let results = index.search("alpha", 3);
        let ids: Vec<usize> = results.iter().map(|r| r.0).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn binary_round_trip_and_determinism() {
        let texts = vec!["hello world", "foo bar baz", "hello foo", "东京 hello"];
        let a = Bm25Index::build(&texts);
        let bytes_a = a.to_bytes().unwrap();
        let b = Bm25Index::build(&texts);
        let bytes_b = b.to_bytes().unwrap();
        assert_eq!(
            bytes_a, bytes_b,
            "identical content must give identical bytes"
        );

        let restored = Bm25Index::from_bytes(&bytes_a, texts.len(), Bm25Params::default()).unwrap();
        assert_eq!(restored, a);
        assert_eq!(restored.search("hello", 4), a.search("hello", 4));
        assert!(restored.terms.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn from_bytes_rejects_bad_doc_count_and_out_of_range_postings() {
        let index = Bm25Index::build(&["hello world", "foo bar"]);
        let bytes = index.to_bytes().unwrap();
        assert!(Bm25Index::from_bytes(&bytes, 3, Bm25Params::default()).is_err());

        // Corrupt the first posting doc id of the first term ("bar") to 200.
        let mut bad = bytes.clone();
        // Layout: num_docs(4) avg(4) doc_lengths(8) terms(4) len(2) "bar"(3) count(4) varint...
        let offset = 4 + 4 + 8 + 4 + 2 + 3 + 4;
        bad[offset] = 200;
        assert!(Bm25Index::from_bytes(&bad, 2, Bm25Params::default()).is_err());

        // Any truncation must be an error, never a panic.
        for cut in 0..bytes.len() {
            assert!(Bm25Index::from_bytes(&bytes[..cut], 2, Bm25Params::default()).is_err());
        }
    }

    #[test]
    fn varint_round_trip() {
        for v in [0u32, 1, 127, 128, 300, 16_383, 16_384, u32::MAX] {
            let mut out = Vec::new();
            write_varint(&mut out, v);
            let mut c = ByteCursor::new(&out);
            assert_eq!(c.varint().unwrap(), v);
            assert_eq!(c.remaining(), 0);
        }
        let mut c = ByteCursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
        assert!(c.varint().is_err());
    }
}
