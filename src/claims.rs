// SPDX-License-Identifier: GPL-3.0-only

//! Build-time factual claim extraction and human-friendly claim edits.

use std::collections::HashSet;
use std::sync::LazyLock;

#[cfg(not(target_arch = "wasm32"))]
use anyhow::{Context, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::chunk::ChunkMeta;

const DEFAULT_SUBJECT: &str = "Subject";

/// Confidence assigned to every heuristic (regex-extracted) claim. Heuristic
/// extraction has no calibrated notion of confidence, so all entries it
/// produces get the same advisory value rather than hand-picked numbers
/// that looked precise but were not measured.
const HEURISTIC_CONFIDENCE: f32 = 0.5;

const ACTIVITY_WHITELIST: &str = "programming|coding|consulting|engineering|building software";

static YEARS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:i|he|she|they|the subject|(?-i:[A-Z][A-Za-z]+(?:\s+[A-Z][A-Za-z]+)?))\s+has\s+been\s+(?P<activity>{})\s+for\s+(?P<duration>\d{{1,2}}\+?\s*years?)",
        ACTIVITY_WHITELIST
    ))
    .unwrap()
});
static SINCE_AGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?P<activity>{})\b[^.\n]{{0,80}}?\bsince age\s+(?P<age>\d{{1,2}})",
        ACTIVITY_WHITELIST
    ))
    .unwrap()
});

/// An employment cue that, together with a generic capitalised-word subject
/// (as opposed to a pronoun or "the subject"), is required before "worked
/// for"/"worked at" is accepted as an employment claim. Without this, any
/// sentence-initial capitalised common word ("This trick worked for
/// Chrome...") reads as a subject and produces a false employment claim.
static EMPLOYMENT_CUE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(employ(?:ed|ee|er)?|joined|hired|role|contract(?:or)?|position|title|years?)\b",
    )
    .unwrap()
});

// High-precision patterns only. Avoid broad "worked with ..." captures that
// often represent tooling/platform usage rather than employment history.
static WORKED_FOR_OR_AT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?P<subject>i|we|he|she|they|the subject|(?-i:[A-Z][A-Za-z]+(?:\s+[A-Z][A-Za-z]+)?))\b[^.\n]{0,80}\bworked\s+(?:for|at)\s+(?P<orgs>[^.\n]{3,260})",
    )
    .unwrap()
});
static WORKED_FOR_OR_WITH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:i|we|he|she|they|the subject|(?-i:[A-Z][A-Za-z]+(?:\s+[A-Z][A-Za-z]+)?))\b[^.\n]{0,80}\bworked\s+for\s+or\s+with[:\s]+(?P<orgs>[^.\n]{3,260})",
    )
    .unwrap()
});

// Resume heading style: "Sr. Engineer at Common Crawl Foundation (1 year contract)"
static ROLE_AT_ORG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^\s*(?:[-*]\s*)?(?:\d+\.\s*)?(?:#+\s*)?(?:[A-Za-z][A-Za-z0-9,&/.+:' -]{1,90})\s+at\s+(?P<org>[A-Z][A-Za-z0-9&/.+,' -]{1,100})\s*(?:\(|$)",
    )
    .unwrap()
});
// Resume heading style: "Kagi.com (consulting)"
static ORG_WITH_CONTEXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^\s*(?:[-*]\s*)?(?:\d+\.\s*)?(?:#+\s*)?(?P<org>[A-Z][A-Za-z0-9&/.+,' -]{1,100})\s*\((?:consulting|contract|freelance|advisor|advisory|part[- ]time)[^)]*\)\s*$",
    )
    .unwrap()
});
// Numbered bullet style: "1. Kagi - semantic search ..."
static NUMBERED_ORG_DASH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*\d+\.\s+(?P<org>[A-Z][A-Za-z0-9&/.+,' ]{1,100}?)\s+-\s+").unwrap()
});

static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*[-*]\s+(?P<item>[^\n]{1,100})\s*$").unwrap());
static LABELED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)\b(?:skills?|technologies|tools|frameworks?|languages?)\s*[:\-]\s*(?P<items>[^\n]{3,260})")
        .unwrap()
});
static EXPERIENCE_PHRASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)\b(?:experience with|familiar with|proficient in|knowledge of)\s+(?P<items>[^\n\.;]{3,220})")
        .unwrap()
});

static SENTENCE_SPLITTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\n.!?]+\s*").unwrap());

pub use crate::records::ClaimEntry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimCorpus {
    pub version: u32,
    pub claims: Vec<ClaimEntry>,
}

impl ClaimCorpus {
    pub fn dedup(&mut self) {
        let mut seen = HashSet::new();
        self.claims.retain(|claim| {
            let key = format!(
                "{}||{}||{}||{}",
                claim.subject.trim().to_lowercase(),
                claim.predicate.trim().to_lowercase(),
                claim.object.trim().to_lowercase(),
                claim.source_url.trim().to_lowercase(),
            );
            seen.insert(key)
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimsEdits {
    #[serde(default)]
    pub add: Vec<ClaimAdd>,
    #[serde(default)]
    pub redact: Vec<ClaimRedact>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAdd {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub source_title: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub source_section: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRedact {
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub predicate: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub contains: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ClaimRedact {
    fn has_criteria(&self) -> bool {
        self.subject.is_some()
            || self.predicate.is_some()
            || self.object.is_some()
            || self.source_url.is_some()
            || self.contains.is_some()
    }
}

pub fn build_claim_corpus_from_chunks(texts: &[String], metadata: &[ChunkMeta]) -> ClaimCorpus {
    let mut claims = Vec::new();
    for (idx, text) in texts.iter().enumerate() {
        if let Some(meta) = metadata.get(idx) {
            claims.extend(extract_claims_from_chunk(text, meta));
        }
    }

    let mut corpus = ClaimCorpus { version: 1, claims };
    corpus.dedup();
    corpus
}

pub fn extract_claims_from_chunk(text: &str, meta: &ChunkMeta) -> Vec<ClaimEntry> {
    let mut out = Vec::new();
    out.extend(extract_experience_claims(text, meta));
    out.extend(extract_work_history_claims(text, meta));
    out.extend(extract_role_company_claims(text, meta));
    out.extend(extract_skill_claims(text, meta));
    out
}

#[cfg(not(target_arch = "wasm32"))]
pub fn parse_claim_edits_toml(raw: &str) -> anyhow::Result<ClaimsEdits> {
    let edits: ClaimsEdits = toml::from_str(raw).context("parsing claims edits TOML")?;
    for (i, redact) in edits.redact.iter().enumerate() {
        if !redact.has_criteria() {
            bail!(
                "claims edits: [[redact]] entry #{} has no matching criteria (subject/predicate/object/source_url/contains set); refusing to apply an unconditional redact",
                i + 1
            );
        }
    }
    Ok(edits)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn apply_claim_edits(claims: &mut Vec<ClaimEntry>, edits: &ClaimsEdits) {
    if !edits.redact.is_empty() {
        claims.retain(|claim| !edits.redact.iter().any(|r| redact_matches(r, claim)));
    }

    for add in &edits.add {
        claims.push(ClaimEntry {
            subject: add.subject.clone(),
            predicate: add.predicate.clone(),
            object: add.object.clone(),
            evidence: add
                .evidence
                .clone()
                .unwrap_or_else(|| "manual claim addition".to_string()),
            source_title: add
                .source_title
                .clone()
                .unwrap_or_else(|| "manual".to_string()),
            source_url: add
                .source_url
                .clone()
                .unwrap_or_else(|| "/manual/".to_string()),
            source_section: add.source_section.clone(),
            tags: if add.tags.is_empty() {
                vec!["manual".to_string()]
            } else {
                add.tags.clone()
            },
            // Manual adds keep whatever confidence the user set; default to
            // 1.0 (fully trusted) only when they did not specify one.
            confidence: add.confidence.unwrap_or(1.0),
        });
    }

    let mut corpus = ClaimCorpus {
        version: 1,
        claims: std::mem::take(claims),
    };
    corpus.dedup();
    *claims = corpus.claims;
}

#[cfg(not(target_arch = "wasm32"))]
fn redact_matches(redact: &ClaimRedact, claim: &ClaimEntry) -> bool {
    let mut matched_any = false;

    if let Some(subject) = &redact.subject {
        matched_any = true;
        if !eq_ci(&claim.subject, subject) {
            return false;
        }
    }
    if let Some(predicate) = &redact.predicate {
        matched_any = true;
        if !eq_ci(&claim.predicate, predicate) {
            return false;
        }
    }
    if let Some(object) = &redact.object {
        matched_any = true;
        if !eq_ci(&claim.object, object) {
            return false;
        }
    }
    if let Some(source_url) = &redact.source_url {
        matched_any = true;
        if !eq_ci(&claim.source_url, source_url) {
            return false;
        }
    }
    if let Some(contains) = &redact.contains {
        matched_any = true;
        let needle = contains.to_lowercase();
        let hay = format!(
            "{} {} {} {}",
            claim.subject, claim.predicate, claim.object, claim.evidence
        )
        .to_lowercase();
        if !hay.contains(&needle) {
            return false;
        }
    }

    matched_any
}

fn extract_experience_claims(text: &str, meta: &ChunkMeta) -> Vec<ClaimEntry> {
    let mut out = Vec::new();

    for sentence in split_sentences(text) {
        for cap in YEARS_RE.captures_iter(sentence) {
            let activity =
                normalize_activity(cap.name("activity").map(|m| m.as_str()).unwrap_or(""));
            let duration = cap
                .name("duration")
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            if activity.is_empty() || duration.is_empty() {
                continue;
            }
            out.push(make_claim(
                DEFAULT_SUBJECT.to_string(),
                format!("years_{}", activity),
                duration,
                sentence,
                meta,
                vec!["experience".to_string(), activity.to_string()],
                HEURISTIC_CONFIDENCE,
            ));
        }

        for cap in SINCE_AGE_RE.captures_iter(sentence) {
            let activity =
                normalize_activity(cap.name("activity").map(|m| m.as_str()).unwrap_or(""));
            let age = cap
                .name("age")
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            if activity.is_empty() || age.is_empty() {
                continue;
            }
            out.push(make_claim(
                DEFAULT_SUBJECT.to_string(),
                format!("since_age_{}", activity),
                age,
                sentence,
                meta,
                vec!["experience".to_string(), activity.to_string()],
                HEURISTIC_CONFIDENCE,
            ));
        }
    }

    out
}

/// Whether the matched subject text is a strong, low-false-positive-rate
/// employment subject (a pronoun or the fixed "the subject" placeholder).
/// Anything else fell through to the generic capitalised-word alternative,
/// which also matches ordinary sentence-initial capitalisation ("This
/// trick worked for..."), so those matches additionally require an
/// employment cue elsewhere in the sentence.
fn is_employment_subject(subject: &str) -> bool {
    matches!(
        subject.to_lowercase().as_str(),
        "i" | "we" | "he" | "she" | "they" | "the subject"
    )
}

fn extract_work_history_claims(text: &str, meta: &ChunkMeta) -> Vec<ClaimEntry> {
    let mut out = Vec::new();

    for sentence in split_sentences(text) {
        for cap in WORKED_FOR_OR_AT_RE.captures_iter(sentence) {
            let subject = cap.name("subject").map(|m| m.as_str()).unwrap_or("");
            if !is_employment_subject(subject) && !EMPLOYMENT_CUE_RE.is_match(sentence) {
                continue;
            }
            push_worked_for_orgs(
                &mut out,
                cap.name("orgs").map(|m| m.as_str()).unwrap_or(""),
                sentence,
                meta,
            );
        }
        for cap in WORKED_FOR_OR_WITH_RE.captures_iter(sentence) {
            push_worked_for_orgs(
                &mut out,
                cap.name("orgs").map(|m| m.as_str()).unwrap_or(""),
                sentence,
                meta,
            );
        }
    }

    out
}

fn push_worked_for_orgs(
    out: &mut Vec<ClaimEntry>,
    orgs_raw: &str,
    sentence: &str,
    meta: &ChunkMeta,
) {
    for org in split_orgs(orgs_raw) {
        out.push(make_claim(
            DEFAULT_SUBJECT.to_string(),
            "worked_for".to_string(),
            org,
            sentence,
            meta,
            vec!["work-history".to_string()],
            HEURISTIC_CONFIDENCE,
        ));
    }
}

fn extract_role_company_claims(text: &str, meta: &ChunkMeta) -> Vec<ClaimEntry> {
    let mut out = Vec::new();

    for cap in ROLE_AT_ORG_RE.captures_iter(text) {
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        if let Some(org) = cap
            .name("org")
            .and_then(|m| clean_org_candidate(m.as_str()))
        {
            out.push(make_claim(
                DEFAULT_SUBJECT.to_string(),
                "worked_for".to_string(),
                org,
                evidence,
                meta,
                vec!["work-history".to_string(), "role-line".to_string()],
                HEURISTIC_CONFIDENCE,
            ));
        }
    }

    for cap in ORG_WITH_CONTEXT_RE.captures_iter(text) {
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        if let Some(org) = cap
            .name("org")
            .and_then(|m| clean_org_candidate(m.as_str()))
        {
            out.push(make_claim(
                DEFAULT_SUBJECT.to_string(),
                "worked_for".to_string(),
                org,
                evidence,
                meta,
                vec!["work-history".to_string(), "role-line".to_string()],
                HEURISTIC_CONFIDENCE,
            ));
        }
    }

    for cap in NUMBERED_ORG_DASH_RE.captures_iter(text) {
        let evidence = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        if let Some(org) = cap
            .name("org")
            .and_then(|m| clean_org_candidate(m.as_str()))
        {
            out.push(make_claim(
                DEFAULT_SUBJECT.to_string(),
                "worked_for".to_string(),
                org,
                evidence,
                meta,
                vec!["work-history".to_string(), "numbered-list".to_string()],
                HEURISTIC_CONFIDENCE,
            ));
        }
    }

    out
}

fn extract_skill_claims(text: &str, meta: &ChunkMeta) -> Vec<ClaimEntry> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let context = format!(
        "{} {} {}",
        meta.title.to_lowercase(),
        meta.section.as_deref().unwrap_or("").to_lowercase(),
        meta.url.to_lowercase(),
    );
    let text_lower = text.to_lowercase();
    let skill_context = context.contains("skill")
        || context.contains("technology")
        || context.contains("tool")
        || context.contains("framework")
        || context.contains("language")
        || context.contains("machine learning")
        || context.contains("/skills/");
    let lexical_hint = text_lower.contains("skills:")
        || text_lower.contains("technologies:")
        || text_lower.contains("tools:")
        || text_lower.contains("frameworks:")
        || text_lower.contains("languages:")
        || text_lower.contains("experience with")
        || text_lower.contains("familiar with")
        || text_lower.contains("proficient in")
        || text_lower.contains("knowledge of");

    if !skill_context && !lexical_hint {
        return out;
    }

    for cap in BULLET_RE.captures_iter(text) {
        let raw = cap.name("item").map(|m| m.as_str()).unwrap_or("");
        if let Some(skill) = clean_skill_candidate(raw) {
            push_skill_claim(
                &mut out,
                &mut seen,
                skill,
                raw,
                meta,
                vec!["skills".to_string(), "bullet-list".to_string()],
                HEURISTIC_CONFIDENCE,
            );
        }
    }

    for cap in LABELED_RE.captures_iter(text) {
        let raw = cap.name("items").map(|m| m.as_str()).unwrap_or("");
        for skill in split_skill_items(raw) {
            push_skill_claim(
                &mut out,
                &mut seen,
                skill.clone(),
                raw,
                meta,
                vec!["skills".to_string(), "labeled-list".to_string()],
                HEURISTIC_CONFIDENCE,
            );
        }
    }

    for cap in EXPERIENCE_PHRASE_RE.captures_iter(text) {
        let raw = cap.name("items").map(|m| m.as_str()).unwrap_or("");
        for skill in split_skill_items(raw) {
            push_skill_claim(
                &mut out,
                &mut seen,
                skill.clone(),
                raw,
                meta,
                vec!["skills".to_string(), "experience-phrase".to_string()],
                HEURISTIC_CONFIDENCE,
            );
        }
    }

    out
}

fn split_orgs(raw: &str) -> Vec<String> {
    raw.replace(" and ", ",")
        .replace(" & ", ",")
        .split(',')
        .filter_map(clean_org_candidate)
        .collect()
}

fn clean_org_candidate(input: &str) -> Option<String> {
    let mut s = input
        .trim()
        .trim_matches(|c: char| c == '.' || c == ';' || c == ':' || c == '-' || c == '*')
        .trim()
        .to_string();

    if s.is_empty() {
        return None;
    }

    for prefix in [
        "or with:",
        "with:",
        "for:",
        "or with",
        "with ",
        "for ",
        "or ",
        "and ",
        "such as ",
        "companies like ",
        "company like ",
        "clients like ",
        "banks like ",
        "healthcare like ",
        "organizations like ",
        "orgs like ",
    ] {
        if s.to_lowercase().starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
        }
    }

    if let Some(pos) = s.find(':')
        && pos < 24
    {
        s = s[pos + 1..].trim().to_string();
    }

    if let Some(pos) = s.find(" (") {
        s = s[..pos].trim().to_string();
    }

    s = s
        .trim_matches(|c: char| c == '.' || c == ';' || c == ',' || c == '(' || c == ')')
        .trim()
        .to_string();

    if s.is_empty() {
        return None;
    }

    if s.eq_ignore_ascii_case("etc") || s.eq_ignore_ascii_case("and more") {
        return None;
    }

    if s.split_whitespace().count() > 8 {
        return None;
    }

    let has_upper_or_digit = s
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !has_upper_or_digit {
        return None;
    }

    if let Some(first) = s.chars().next()
        && !first.is_ascii_uppercase()
        && !first.is_ascii_digit()
    {
        return None;
    }

    let lowercase_leads = s
        .split_whitespace()
        .filter(|token| token.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
        .count();
    if lowercase_leads > 1 {
        return None;
    }

    // Reject candidates that end in a lowercase common-noun plural
    // ("Windows 10 users", "Acme Corp employees"): a real organization name
    // does not end with an ordinary plural noun describing a group of
    // people or things, so this is a strong signal that "worked for" was
    // used in its everyday sense ("the fix worked for these users") rather
    // than an employment sense.
    if let Some(last_word) = s.split_whitespace().last() {
        let stripped = last_word.trim_end_matches(|c: char| !c.is_alphanumeric());
        let starts_lowercase = stripped
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase());
        if starts_lowercase
            && stripped.len() > 2
            && stripped.ends_with('s')
            && !stripped.ends_with("ss")
        {
            return None;
        }
    }

    let lower = s.to_lowercase();
    if lower.contains("they had")
        || lower.contains("needed ")
        || lower.contains("behavior ")
        || lower.contains("services")
        || lower.contains(" not ")
        || lower.contains("events ")
        || lower.contains("head of ")
    {
        return None;
    }

    Some(s)
}

fn split_skill_items(raw: &str) -> Vec<String> {
    raw.replace(" and ", ",")
        .replace(" / ", ",")
        .split(',')
        .filter_map(clean_skill_candidate)
        .collect()
}

fn clean_skill_candidate(input: &str) -> Option<String> {
    let mut s = strip_markdown_link(input);
    s = s.replace('`', "");
    s = s
        .trim()
        .trim_matches(|c: char| {
            c == '.'
                || c == ';'
                || c == ':'
                || c == '-'
                || c == '*'
                || c == '('
                || c == ')'
                || c == '"'
                || c == '\''
        })
        .trim()
        .to_string();
    if s.is_empty() {
        return None;
    }

    if s.len() > 48 || s.split_whitespace().count() > 6 {
        return None;
    }
    let lower = s.to_lowercase();
    if lower.contains("http://")
        || lower.contains("https://")
        || lower.starts_with("i have ")
        || lower.starts_with("i've ")
        || lower.contains(" years")
        || lower.contains("age ")
        || lower.contains("worked ")
        || lower.contains("consult")
    {
        return None;
    }

    let has_alpha = s.chars().any(|c| c.is_ascii_alphabetic());
    if !has_alpha {
        return None;
    }

    Some(normalize_ws(&s))
}

fn strip_markdown_link(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(')') {
        return trimmed.to_string();
    }
    let Some(mid) = trimmed.find("](") else {
        return trimmed.to_string();
    };
    let label = &trimmed[1..mid];
    if label.trim().is_empty() {
        return trimmed.to_string();
    }
    label.trim().to_string()
}

fn push_skill_claim(
    out: &mut Vec<ClaimEntry>,
    seen: &mut HashSet<String>,
    skill: String,
    evidence: &str,
    meta: &ChunkMeta,
    tags: Vec<String>,
    confidence: f32,
) {
    let key = skill.to_lowercase();
    if !seen.insert(key) {
        return;
    }
    out.push(make_claim(
        DEFAULT_SUBJECT.to_string(),
        "has_skill".to_string(),
        skill,
        evidence,
        meta,
        tags,
        confidence,
    ));
}

fn make_claim(
    subject: String,
    predicate: String,
    object: String,
    evidence: &str,
    meta: &ChunkMeta,
    tags: Vec<String>,
    confidence: f32,
) -> ClaimEntry {
    ClaimEntry {
        subject,
        predicate,
        object,
        evidence: normalize_sentence(evidence),
        source_title: meta.title.clone(),
        source_url: meta.url.clone(),
        source_section: meta.section.clone(),
        tags,
        confidence,
    }
}

fn normalize_sentence(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.ends_with('.') || trimmed.ends_with('!') || trimmed.ends_with('?') {
        trimmed.to_string()
    } else {
        format!("{}.", trimmed)
    }
}

fn split_sentences(text: &str) -> Vec<&str> {
    SENTENCE_SPLITTER_RE
        .split(text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn normalize_activity(activity: &str) -> &str {
    let lower = activity.trim().to_lowercase();
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

fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(not(target_arch = "wasm32"))]
fn eq_ci(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn extract_worked_for_claims() {
        let text = "The subject worked for Life Time Fitness, Common Crawl, Kagi, and Nike.";
        let claims = extract_claims_from_chunk(text, &meta());
        let employers: Vec<String> = claims
            .iter()
            .filter(|c| c.predicate == "worked_for")
            .map(|c| c.object.clone())
            .collect();
        assert!(employers.iter().any(|o| o == "Life Time Fitness"));
        assert!(employers.iter().any(|o| o == "Common Crawl"));
        assert!(employers.iter().any(|o| o == "Kagi"));
        assert!(employers.iter().any(|o| o == "Nike"));
    }

    #[test]
    fn extract_worked_for_or_with_list_claims() {
        let text = "Other companies of note I've worked for or with: Target (ecom and innovation groups), Nike, 5/3 Bank, United Health Group, Kenneth Cole, and Bauer Hockey.";
        let claims = extract_claims_from_chunk(text, &meta());
        let employers: Vec<String> = claims
            .iter()
            .filter(|c| c.predicate == "worked_for")
            .map(|c| c.object.clone())
            .collect();
        assert!(employers.iter().any(|o| o == "Target"));
        assert!(employers.iter().any(|o| o == "Nike"));
        assert!(employers.iter().any(|o| o == "5/3 Bank"));
        assert!(employers.iter().any(|o| o == "United Health Group"));
        assert!(employers.iter().any(|o| o == "Kenneth Cole"));
        assert!(employers.iter().any(|o| o == "Bauer Hockey"));
    }

    #[test]
    fn extract_resume_heading_work_claims() {
        let text = r#"
### Kagi.com (consulting)
### Sr. Engineer at Common Crawl Foundation (1 year contract)
### Sr. Vice President, Chief of Technology at Life Time Fitness
"#;
        let claims = extract_claims_from_chunk(text, &meta());
        let employers: Vec<String> = claims
            .iter()
            .filter(|c| c.predicate == "worked_for")
            .map(|c| c.object.clone())
            .collect();
        assert!(employers.iter().any(|o| o == "Kagi.com"));
        assert!(employers.iter().any(|o| o == "Common Crawl Foundation"));
        assert!(employers.iter().any(|o| o == "Life Time Fitness"));
    }

    #[test]
    fn ignores_non_employment_worked_with_usage() {
        let text = "When I first worked with AWS, they had 3 services...";
        let claims = extract_claims_from_chunk(text, &meta());
        assert!(claims.iter().all(|c| c.predicate != "worked_for"));
    }

    #[test]
    fn rejects_worked_for_in_non_employment_sense() {
        let text = "This trick worked for Chrome, Firefox, and Safari.";
        let claims = extract_claims_from_chunk(text, &meta());
        assert!(claims.iter().all(|c| c.predicate != "worked_for"));
    }

    #[test]
    fn rejects_worked_for_object_ending_in_plural_common_noun() {
        let text = "The fix worked for Windows 10 users.";
        let claims = extract_claims_from_chunk(text, &meta());
        assert!(claims.iter().all(|c| c.predicate != "worked_for"));
    }

    #[test]
    fn rejects_worked_for_object_with_trailing_plural_common_noun_even_with_strong_subject() {
        let text = "I worked for Acme Corp employees.";
        let claims = extract_claims_from_chunk(text, &meta());
        assert!(claims.iter().all(|c| c.predicate != "worked_for"));
    }

    #[test]
    fn extract_programming_year_claim() {
        let text = "The subject has been programming for 42 years.";
        let claims = extract_claims_from_chunk(text, &meta());
        assert!(
            claims
                .iter()
                .any(|c| c.predicate == "years_programming" && c.object.contains("42"))
        );
    }

    #[test]
    fn parse_and_apply_claim_edits() {
        let mut claims = vec![ClaimEntry {
            subject: "Subject".to_string(),
            predicate: "worked_for".to_string(),
            object: "Foo Corp".to_string(),
            evidence: "The subject worked for Foo Corp".to_string(),
            source_title: "About".to_string(),
            source_url: "/about/".to_string(),
            source_section: None,
            tags: vec!["work-history".to_string()],
            confidence: 0.8,
        }];

        let toml = r#"
[[redact]]
predicate = "worked_for"
object = "Foo Corp"

[[add]]
subject = "Subject"
predicate = "worked_for"
object = "Nike"
evidence = "Manual correction"
source_url = "/about/"
confidence = 1.0
"#;
        let edits = parse_claim_edits_toml(toml).unwrap();
        apply_claim_edits(&mut claims, &edits);

        assert!(claims.iter().all(|c| c.object != "Foo Corp"));
        assert!(
            claims
                .iter()
                .any(|c| c.predicate == "worked_for" && c.object == "Nike")
        );
    }

    #[test]
    fn rejects_empty_redact_block() {
        let toml = "[[redact]]\n";
        assert!(parse_claim_edits_toml(toml).is_err());
    }

    #[test]
    fn rejects_unknown_fields_in_edits() {
        let toml = r#"
[[add]]
subject = "Subject"
predicate = "worked_for"
object = "Nike"
typo_field = "oops"
"#;
        assert!(parse_claim_edits_toml(toml).is_err());
    }

    #[test]
    fn extract_skill_claims_from_skill_like_page() {
        let mut m = meta();
        m.title = "Machine Learning".to_string();
        m.url = "/skills/machine-learning/".to_string();
        let text = r#"
* K-Means
* Markov Models
* Bayesian networks
"#;
        let claims = extract_claims_from_chunk(text, &m);
        let skills: Vec<String> = claims
            .iter()
            .filter(|c| c.predicate == "has_skill")
            .map(|c| c.object.clone())
            .collect();
        assert!(skills.iter().any(|s| s == "K-Means"));
        assert!(skills.iter().any(|s| s == "Markov Models"));
        assert!(skills.iter().any(|s| s == "Bayesian networks"));
    }
}
