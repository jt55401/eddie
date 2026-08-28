// SPDX-License-Identifier: GPL-3.0-only

//! External acceptance-suite evaluation for site-specific search quality.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AcceptanceSuite {
    #[serde(default)]
    pub name: Option<String>,
    pub cases: Vec<AcceptanceCase>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AcceptanceCase {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub must_match_any: Vec<String>,
    #[serde(default)]
    pub must_include_all: Vec<String>,
    #[serde(default)]
    pub top_k: Option<usize>,
    #[serde(default = "default_weight")]
    pub weight: f32,
    #[serde(default)]
    pub user_rating: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseEvaluation {
    pub id: String,
    pub passed: bool,
    pub score: f32,
    pub matched_any: Option<String>,
    pub missing_all: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SuiteEvaluation {
    pub passed_cases: usize,
    pub total_cases: usize,
    pub pass_rate: f32,
    pub weighted_score: f32,
    pub weighted_total: f32,
    pub cases: Vec<CaseEvaluation>,
}

impl AcceptanceSuite {
    pub fn validate(&self) -> Result<()> {
        if self.cases.is_empty() {
            bail!("acceptance suite must contain at least one case");
        }

        for case in &self.cases {
            if case.query.trim().is_empty() {
                bail!("case '{}' has an empty query", case.id);
            }
            if case.must_match_any.is_empty() && case.must_include_all.is_empty() {
                bail!(
                    "case '{}' must define at least one matcher in must_match_any or must_include_all",
                    case.id
                );
            }
            if case.weight <= 0.0 {
                bail!("case '{}' has non-positive weight", case.id);
            }
            if let Some(rating) = case.user_rating
                && !(1..=5).contains(&rating)
            {
                bail!("case '{}' has user_rating outside 1..=5", case.id);
            }
        }

        Ok(())
    }
}

pub fn load_suite(path: &Path) -> Result<AcceptanceSuite> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading acceptance suite {}", path.display()))?;
    let suite: AcceptanceSuite = serde_json::from_str(&raw)
        .with_context(|| format!("parsing acceptance suite {} as JSON", path.display()))?;
    suite.validate()?;
    Ok(suite)
}

pub fn write_suite(path: &Path, suite: &AcceptanceSuite) -> Result<()> {
    let json = serde_json::to_string_pretty(suite).context("serializing acceptance suite")?;
    fs::write(path, json).with_context(|| format!("writing suite to {}", path.display()))
}

pub fn evaluate_case(case: &AcceptanceCase, retrieved_text: &str) -> CaseEvaluation {
    let normalized = normalize(retrieved_text);

    let mut matched_any = None;
    let any_ok = if case.must_match_any.is_empty() {
        true
    } else {
        let mut found = false;
        for needle in &case.must_match_any {
            let n = normalize(needle);
            if !n.is_empty() && normalized.contains(&n) {
                found = true;
                matched_any = Some(needle.clone());
                break;
            }
        }
        found
    };

    let mut missing_all = Vec::new();
    for needle in &case.must_include_all {
        let n = normalize(needle);
        if n.is_empty() {
            continue;
        }
        if !normalized.contains(&n) {
            missing_all.push(needle.clone());
        }
    }

    let passed = any_ok && missing_all.is_empty();
    let score = if passed { case.weight } else { 0.0 };

    CaseEvaluation {
        id: case.id.clone(),
        passed,
        score,
        matched_any,
        missing_all,
    }
}

pub fn summarize(cases: Vec<CaseEvaluation>, suite: &AcceptanceSuite) -> SuiteEvaluation {
    let passed_cases = cases.iter().filter(|c| c.passed).count();
    let total_cases = cases.len();
    let pass_rate = if total_cases == 0 {
        0.0
    } else {
        passed_cases as f32 / total_cases as f32
    };

    let weighted_total: f32 = suite.cases.iter().map(|c| c.weight).sum();
    let weighted_score: f32 = cases.iter().map(|c| c.score).sum();

    SuiteEvaluation {
        passed_cases,
        total_cases,
        pass_rate,
        weighted_score,
        weighted_total,
        cases,
    }
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_weight() -> f32 {
    1.0
}

/// Hit@k: 1.0 if any of the first `k` retrieved ids appears in `relevant`,
/// else 0.0. Returns 0.0 (never NaN or a divide-by-zero panic) when
/// `relevant` is empty, `retrieved` is empty, or `k` is 0 — there is
/// nothing to hit in any of those cases.
pub fn hit_at_k(retrieved: &[String], relevant: &[String], k: usize) -> f64 {
    if relevant.is_empty() || retrieved.is_empty() || k == 0 {
        return 0.0;
    }
    let top = &retrieved[..retrieved.len().min(k)];
    if top.iter().any(|r| relevant.iter().any(|rel| rel == r)) {
        1.0
    } else {
        0.0
    }
}

/// Reciprocal rank of the first relevant id in `retrieved` (1-indexed), or
/// 0.0 if none is found or `relevant` is empty. Callers average this across
/// queries to get a mean reciprocal rank (MRR).
pub fn mrr(retrieved: &[String], relevant: &[String]) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    for (i, r) in retrieved.iter().enumerate() {
        if relevant.iter().any(|rel| rel == r) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Normalized Discounted Cumulative Gain at `k`, using binary relevance.
/// Returns 0.0 when `relevant` is empty or `k` is 0 rather than dividing by
/// zero (there is no possible gain to normalize against).
pub fn ndcg_at_k(retrieved: &[String], relevant: &[String], k: usize) -> f64 {
    if relevant.is_empty() || k == 0 {
        return 0.0;
    }

    let top = &retrieved[..retrieved.len().min(k)];
    let dcg: f64 = top
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let gain = if relevant.iter().any(|rel| rel == r) {
                1.0
            } else {
                0.0
            };
            gain / (i as f64 + 2.0).log2()
        })
        .sum();

    let ideal_hits = relevant.len().min(k);
    let idcg: f64 = (0..ideal_hits).map(|i| 1.0 / (i as f64 + 2.0).log2()).sum();

    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_case_match_any_passes() {
        let case = AcceptanceCase {
            id: "programming-years".to_string(),
            query: "how many years has the subject been programming".to_string(),
            must_match_any: vec!["40+".to_string(), "since age 6".to_string()],
            must_include_all: vec![],
            top_k: None,
            weight: 1.0,
            user_rating: None,
        };

        let out = evaluate_case(&case, "The subject has been programming since age 6.");
        assert!(out.passed);
        assert_eq!(out.matched_any.as_deref(), Some("since age 6"));
    }

    #[test]
    fn evaluate_case_missing_required_phrase_fails() {
        let case = AcceptanceCase {
            id: "worked-for".to_string(),
            query: "who has the subject worked for".to_string(),
            must_match_any: vec!["common crawl".to_string()],
            must_include_all: vec!["kagi".to_string()],
            top_k: Some(5),
            weight: 2.0,
            user_rating: Some(5),
        };

        let out = evaluate_case(&case, "The subject worked for Common Crawl and Nike.");
        assert!(!out.passed);
        assert_eq!(out.missing_all, vec!["kagi".to_string()]);
    }

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn hit_at_k_finds_hit_within_window() {
        let retrieved = ids(&["a", "b", "c"]);
        let relevant = ids(&["c"]);
        assert_eq!(hit_at_k(&retrieved, &relevant, 3), 1.0);
        assert_eq!(hit_at_k(&retrieved, &relevant, 2), 0.0);
    }

    #[test]
    fn hit_at_k_empty_relevant_is_zero_not_nan() {
        let retrieved = ids(&["a", "b"]);
        let relevant: Vec<String> = Vec::new();
        assert_eq!(hit_at_k(&retrieved, &relevant, 5), 0.0);
    }

    #[test]
    fn mrr_scores_reciprocal_of_first_hit_rank() {
        let retrieved = ids(&["a", "b", "c"]);
        let relevant = ids(&["b"]);
        assert!((mrr(&retrieved, &relevant) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn mrr_no_hit_or_empty_relevant_is_zero() {
        let retrieved = ids(&["a", "b"]);
        assert_eq!(mrr(&retrieved, &ids(&["z"])), 0.0);
        assert_eq!(mrr(&retrieved, &[]), 0.0);
    }

    #[test]
    fn ndcg_perfect_ranking_is_one() {
        let retrieved = ids(&["a", "b", "c"]);
        let relevant = ids(&["a", "b"]);
        let score = ndcg_at_k(&retrieved, &relevant, 3);
        assert!((score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ndcg_empty_relevant_is_zero_not_nan() {
        let retrieved = ids(&["a", "b"]);
        let score = ndcg_at_k(&retrieved, &[], 3);
        assert_eq!(score, 0.0);
        assert!(!score.is_nan());
    }

    #[test]
    fn ndcg_zero_k_is_zero() {
        let retrieved = ids(&["a", "b"]);
        let relevant = ids(&["a"]);
        assert_eq!(ndcg_at_k(&retrieved, &relevant, 0), 0.0);
    }

    #[test]
    fn suite_validate_rejects_empty_expectations() {
        let suite = AcceptanceSuite {
            name: Some("x".to_string()),
            cases: vec![AcceptanceCase {
                id: "bad".to_string(),
                query: "query".to_string(),
                must_match_any: vec![],
                must_include_all: vec![],
                top_k: None,
                weight: 1.0,
                user_rating: None,
            }],
        };

        assert!(suite.validate().is_err());
    }
}
