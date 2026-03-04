// SPDX-License-Identifier: GPL-3.0-only

//! Build-time Q&A corpus synthesis from indexed chunks.

use std::collections::HashSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::chunk::ChunkMeta;
use crate::claims::extract_claims_from_chunk;

const SUBJECT_LABEL: &str = "the subject";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaEntry {
    pub question: String,
    pub answer: String,
    pub source_title: String,
    pub source_url: String,
    pub source_section: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaCorpus {
    pub version: u32,
    pub entries: Vec<QaEntry>,
}

impl QaCorpus {
    pub fn dedup(&mut self) {
        let mut seen = HashSet::new();
        self.entries.retain(|entry| {
            let key = format!(
                "{}||{}",
                entry.question.trim().to_lowercase(),
                entry.answer.trim().to_lowercase()
            );
            seen.insert(key)
        });
    }
}

pub fn build_qa_corpus_from_chunks(texts: &[String], metadata: &[ChunkMeta]) -> QaCorpus {
    let mut entries = Vec::new();

    for (i, text) in texts.iter().enumerate() {
        if let Some(meta) = metadata.get(i) {
            entries.extend(extract_from_chunk(text, meta));
        }
    }

    let mut corpus = QaCorpus {
        version: 1,
        entries,
    };
    corpus.dedup();
    corpus
}

pub fn build_qa_entries_from_chunks(texts: &[String], metadata: &[ChunkMeta]) -> Vec<QaEntry> {
    let corpus = build_qa_corpus_from_chunks(texts, metadata);
    corpus.entries
}

pub fn extract_from_chunk(text: &str, meta: &ChunkMeta) -> Vec<QaEntry> {
    let mut entries = Vec::new();

    entries.extend(extract_experience_qa(text, meta));
    entries.extend(extract_work_history_qa(text, meta));
    entries.extend(extract_claim_backed_qa(text, meta));

    entries
}

fn extract_experience_qa(text: &str, meta: &ChunkMeta) -> Vec<QaEntry> {
    let mut out = Vec::new();

    let years_re = Regex::new(
        r"(?i)\b(?P<activity>programming|coding|software engineering|engineering|consulting|developer relations|building software)\b(?P<tail>[^.\n]{0,120})",
    )
    .unwrap();
    let years_count_re = Regex::new(r"(?i)\b(?P<years>\d{1,2}\+?\s*years?)\b").unwrap();
    let since_age_re = Regex::new(r"(?i)\bsince age\s+(?P<age>\d{1,2})\b").unwrap();
    let since_year_re = Regex::new(r"(?i)\bsince\s+(?P<year>19\d{2}|20\d{2})\b").unwrap();
    let has_been_re = Regex::new(
        r"(?i)\b(?:i|he|she|they|the subject|[A-Z][A-Za-z]+(?:\s+[A-Z][A-Za-z]+)?)\s+has\s+been\s+(?P<activity>[a-z][a-z\s\-]{2,40})\s+for\s+(?P<duration>[^\n\.,;]{2,50})",
    )
    .unwrap();

    for sentence in split_sentences(text) {
        let sentence_trimmed = sentence.trim();
        if sentence_trimmed.is_empty() {
            continue;
        }

        for cap in years_re.captures_iter(sentence_trimmed) {
            let activity =
                canonical_activity(cap.name("activity").map(|m| m.as_str()).unwrap_or(""));
            if activity.is_empty() {
                continue;
            }

            let has_duration = years_count_re.is_match(sentence_trimmed)
                || since_age_re.is_match(sentence_trimmed)
                || since_year_re.is_match(sentence_trimmed);

            if !has_duration {
                continue;
            }

            let answer = normalize_sentence(sentence_trimmed);
            out.push(make_entry(
                format!("How many years has {} been {}?", SUBJECT_LABEL, activity),
                answer.clone(),
                meta,
                vec!["experience".to_string(), activity.to_string()],
                0.75,
            ));
            out.push(make_entry(
                format!("How long has {} been {}?", SUBJECT_LABEL, activity),
                answer.clone(),
                meta,
                vec!["experience".to_string(), activity.to_string()],
                0.75,
            ));
            out.push(make_entry(
                format!("Does {} {} very well?", SUBJECT_LABEL, activity),
                answer,
                meta,
                vec!["experience".to_string(), activity.to_string()],
                0.65,
            ));
        }

        for cap in has_been_re.captures_iter(sentence_trimmed) {
            let activity = cap
                .name("activity")
                .map(|m| m.as_str().trim().to_lowercase())
                .unwrap_or_default();
            let duration = cap
                .name("duration")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if activity.is_empty() || duration.is_empty() {
                continue;
            }
            let answer = format!("{} has been {} for {}.", SUBJECT_LABEL, activity, duration);
            out.push(make_entry(
                format!("Does {} {} very well?", SUBJECT_LABEL, activity),
                answer.clone(),
                meta,
                vec!["experience".to_string(), activity.clone()],
                0.8,
            ));
            out.push(make_entry(
                format!("How long has {} been {}?", SUBJECT_LABEL, activity),
                answer,
                meta,
                vec!["experience".to_string(), activity],
                0.8,
            ));
        }
    }

    out
}

fn extract_work_history_qa(text: &str, meta: &ChunkMeta) -> Vec<QaEntry> {
    let mut out = Vec::new();

    let worked_for_re =
        Regex::new(r"(?i)\bworked\s+(?:for|at|with)\s+(?P<orgs>[^.\n]{8,240})").unwrap();

    for cap in worked_for_re.captures_iter(text) {
        let orgs = cap
            .name("orgs")
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .trim_matches(',')
            .trim();

        if orgs.is_empty() {
            continue;
        }

        let answer = format!("{} has worked for {}.", SUBJECT_LABEL, orgs);
        out.push(make_entry(
            format!("Who has {} worked for?", SUBJECT_LABEL),
            answer,
            meta,
            vec!["work-history".to_string()],
            0.8,
        ));
    }

    out
}

fn extract_claim_backed_qa(text: &str, meta: &ChunkMeta) -> Vec<QaEntry> {
    let mut out = Vec::new();
    let claims = extract_claims_from_chunk(text, meta);
    if claims.is_empty() {
        return out;
    }

    let mut worked_for = Vec::new();
    for claim in claims {
        if claim.predicate == "worked_for" && !worked_for.contains(&claim.object) {
            worked_for.push(claim.object);
            continue;
        }

        if let Some(activity) = claim.predicate.strip_prefix("years_") {
            out.push(make_entry(
                format!("How many years has {} been {}?", SUBJECT_LABEL, activity),
                format!("{} has been {} for {}.", SUBJECT_LABEL, activity, claim.object),
                meta,
                vec!["claim-backed".to_string(), "experience".to_string()],
                0.82,
            ));
            continue;
        }

        if let Some(activity) = claim.predicate.strip_prefix("since_age_") {
            out.push(make_entry(
                format!("Since what age has {} been {}?", SUBJECT_LABEL, activity),
                format!(
                    "{} has been {} since age {}.",
                    SUBJECT_LABEL, activity, claim.object
                ),
                meta,
                vec!["claim-backed".to_string(), "experience".to_string()],
                0.8,
            ));
        }
    }

    if !worked_for.is_empty() {
        out.push(make_entry(
            format!("Who has {} worked for?", SUBJECT_LABEL),
            format!("{} has worked for {}.", SUBJECT_LABEL, worked_for.join(", ")),
            meta,
            vec!["claim-backed".to_string(), "work-history".to_string()],
            0.86,
        ));
    }

    out
}

fn make_entry(
    question: String,
    answer: String,
    meta: &ChunkMeta,
    tags: Vec<String>,
    confidence: f32,
) -> QaEntry {
    QaEntry {
        question,
        answer,
        source_title: meta.title.clone(),
        source_url: meta.url.clone(),
        source_section: meta.section.clone(),
        tags,
        confidence,
    }
}

fn split_sentences(text: &str) -> Vec<&str> {
    let splitter = Regex::new(r"[\n\.!?]+\s*").unwrap();
    splitter
        .split(text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn canonical_activity(raw: &str) -> &str {
    let lower = raw.trim().to_lowercase();
    if lower.contains("consult") {
        return "consulting";
    }
    if lower.contains("program") || lower.contains("coding") || lower.contains("software") {
        return "programming";
    }
    if lower.contains("engineer") {
        return "engineering";
    }
    ""
}

fn normalize_sentence(sentence: &str) -> String {
    let trimmed = sentence.trim();
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        trimmed.to_string()
    } else {
        format!("{}.", trimmed)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub model: String,
    pub endpoint: String,
    pub max_chunks: usize,
    pub max_pairs_per_chunk: usize,
    pub temperature: f32,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct OpenRouterConfig {
    pub model: String,
    pub endpoint: String,
    pub api_key: String,
    pub max_chunks: usize,
    pub max_pairs_per_chunk: usize,
    pub temperature: f32,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn synthesize_with_ollama_from_chunks(
    texts: &[String],
    metadata: &[ChunkMeta],
    cfg: &OllamaConfig,
) -> anyhow::Result<Vec<QaEntry>> {
    use anyhow::Context;
    use serde_json::{Value, json};

    let mut out = Vec::new();
    let mut selected = 0usize;

    for (i, text) in texts.iter().enumerate() {
        if selected >= cfg.max_chunks {
            break;
        }
        let Some(meta) = metadata.get(i) else {
            continue;
        };

        if !looks_fact_dense(text) {
            continue;
        }

        selected += 1;

        let prompt = format!(
            "You generate grounded factual Q&A pairs from source text. Return strict JSON only.\\n\\nSource title: {}\\nSource url: {}\\nSource section: {}\\n\\nText:\\n{}\\n\\nReturn this JSON shape exactly:\\n{{\\\"qa\\\":[{{\\\"question\\\":\\\"...\\\",\\\"answer\\\":\\\"...\\\",\\\"tags\\\":[\\\"...\\\"],\\\"confidence\\\":0.0}}]}}\\n\\nRules:\\n- At most {} items.\\n- Only include claims directly supported by the text.\\n- Prefer measurable facts (years, roles, employers, dates).",
            meta.title,
            meta.url,
            meta.section.as_deref().unwrap_or(""),
            text,
            cfg.max_pairs_per_chunk
        );

        let body = json!({
            "model": cfg.model,
            "prompt": prompt,
            "stream": false,
            "options": {
                "temperature": cfg.temperature
            }
        });

        let response: Value = ureq::post(&cfg.endpoint)
            .send_json(body)
            .context("calling ollama generate endpoint")?
            .into_json()
            .context("parsing ollama JSON response")?;

        let response_text = response
            .get("response")
            .and_then(Value::as_str)
            .unwrap_or("");
        if response_text.trim().is_empty() {
            continue;
        }
        out.extend(parse_generated_qa_entries(response_text, meta));
    }

    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn synthesize_with_openrouter_from_chunks(
    texts: &[String],
    metadata: &[ChunkMeta],
    cfg: &OpenRouterConfig,
) -> anyhow::Result<Vec<QaEntry>> {
    use anyhow::Context;
    use serde_json::{Value, json};

    let mut out = Vec::new();
    let mut selected = 0usize;

    for (i, text) in texts.iter().enumerate() {
        if selected >= cfg.max_chunks {
            break;
        }
        let Some(meta) = metadata.get(i) else {
            continue;
        };
        if !looks_fact_dense(text) {
            continue;
        }
        selected += 1;

        let user_prompt = format!(
            "Source title: {}\\nSource url: {}\\nSource section: {}\\n\\nText:\\n{}\\n\\nReturn strict JSON only:\\n{{\\\"qa\\\":[{{\\\"question\\\":\\\"...\\\",\\\"answer\\\":\\\"...\\\",\\\"tags\\\":[\\\"...\\\"],\\\"confidence\\\":0.0}}]}}\\n\\nRules:\\n- At most {} items.\\n- Only include facts directly supported by the source text.\\n- Prefer measurable facts (years, roles, employers, dates).",
            meta.title,
            meta.url,
            meta.section.as_deref().unwrap_or(""),
            text,
            cfg.max_pairs_per_chunk
        );

        let body = json!({
            "model": cfg.model,
            "temperature": cfg.temperature,
            "messages": [
                {
                    "role": "system",
                    "content": "You generate grounded factual Q&A pairs from source text. Output valid JSON only."
                },
                {
                    "role": "user",
                    "content": user_prompt
                }
            ],
            "response_format": { "type": "json_object" }
        });

        let response: Value = ureq::post(&cfg.endpoint)
            .set("Authorization", &format!("Bearer {}", cfg.api_key))
            .set("Content-Type", "application/json")
            .send_json(body)
            .context("calling OpenRouter chat completions endpoint")?
            .into_json()
            .context("parsing OpenRouter JSON response")?;

        let content = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or("");
        if content.trim().is_empty() {
            continue;
        }
        out.extend(parse_generated_qa_entries(content, meta));
    }

    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_generated_qa_entries(response_text: &str, meta: &ChunkMeta) -> Vec<QaEntry> {
    let mut out = Vec::new();

    let parsed: serde_json::Value = match serde_json::from_str(response_text) {
        Ok(v) => v,
        Err(_) => return out,
    };
    let Some(items) = parsed.get("qa").and_then(serde_json::Value::as_array) else {
        return out;
    };

    for item in items {
        let question = item
            .get("question")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        let answer = item
            .get("answer")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if question.is_empty() || answer.is_empty() {
            continue;
        }
        let tags = item
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let confidence = item
            .get("confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.6) as f32;

        out.push(QaEntry {
            question: question.to_string(),
            answer: answer.to_string(),
            source_title: meta.title.clone(),
            source_url: meta.url.clone(),
            source_section: meta.section.clone(),
            tags,
            confidence,
        });
    }

    out
}

#[cfg(not(target_arch = "wasm32"))]
fn looks_fact_dense(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("years")
        || lower.contains("since")
        || lower.contains("worked for")
        || lower.contains("worked at")
        || text.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkMeta;

    fn meta() -> ChunkMeta {
        ChunkMeta {
            title: "About".to_string(),
            url: "/about/".to_string(),
            section: Some("Bio".to_string()),
            date: Some("2024-01-01".to_string()),
            granularity: None,
            chunk_index: 0,
        }
    }

    #[test]
    fn extract_programming_years() {
        let text = "The subject has been programming for 42 years and shipping products.";
        let out = extract_from_chunk(text, &meta());
        assert!(
            out.iter()
                .any(|e| e.question == "How many years has the subject been programming?")
        );
        assert!(out.iter().any(|e| e.answer.contains("42 years")));
    }

    #[test]
    fn extract_since_age() {
        let text = "He has been programming since age 6 across multiple domains.";
        let out = extract_from_chunk(text, &meta());
        assert!(
            out.iter()
                .any(|e| e.answer.to_lowercase().contains("since age 6"))
        );
    }

    #[test]
    fn extract_work_history() {
        let text = "The subject worked for Life Time Fitness, Common Crawl, Kagi, and Nike.";
        let out = extract_from_chunk(text, &meta());
        let who = out
            .iter()
            .find(|e| e.question == "Who has the subject worked for?");
        assert!(who.is_some());
        let answer = &who.unwrap().answer;
        assert!(answer.contains("Life Time Fitness"));
        assert!(answer.contains("Common Crawl"));
        assert!(answer.contains("Kagi"));
        assert!(answer.contains("Nike"));
    }

    #[test]
    fn extract_does_x_very_well_pattern() {
        let text = "The subject has been consulting for 30+ years in enterprise software.";
        let out = extract_from_chunk(text, &meta());
        assert!(
            out.iter()
                .any(|e| e.question == "Does the subject consulting very well?")
        );
        assert!(out.iter().any(|e| e.answer.contains("30+ years")));
    }
}
