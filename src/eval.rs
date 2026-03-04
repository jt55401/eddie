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
            if let Some(rating) = case.user_rating {
                if !(1..=5).contains(&rating) {
                    bail!("case '{}' has user_rating outside 1..=5", case.id);
                }
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
