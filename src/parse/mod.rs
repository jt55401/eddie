// SPDX-License-Identifier: GPL-3.0-only

//! Content parsing for static-site CMSes.

use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use walkdir::WalkDir;

use crate::chunk::{Document, DocumentMeta};

mod astro;
mod docusaurus;
mod eleventy;
mod hugo;
mod jekyll;
mod mkdocs;

pub use astro::AstroParser;
pub use docusaurus::DocusaurusParser;
pub use eleventy::EleventyParser;
pub use hugo::HugoParser;
pub use jekyll::JekyllParser;
pub use mkdocs::MkDocsParser;

/// Trait for CMS-specific content parsing.
pub trait ContentParser {
    /// Parse a file's raw content into metadata and a cleaned body.
    /// Returns `Ok(None)` if the file should be skipped (draft, empty, etc.).
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>>;

    /// File extensions this parser handles.
    fn extensions(&self) -> &[&str] {
        &["md", "markdown"]
    }
}

/// Walk a content directory using the given parser, returning all published documents.
pub fn parse_content_dir(path: &Path, parser: &dyn ContentParser) -> Result<Vec<Document>> {
    let extensions = parser.extensions();
    let mut docs = Vec::new();

    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().is_some_and(|ext| {
                let ext = ext.to_string_lossy().to_ascii_lowercase();
                extensions.iter().any(|x| *x == ext)
            })
        })
    {
        let file_path = entry.path();
        let content = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;

        if let Some((meta, body)) = parser.parse_file(&content, file_path, path)? {
            if !body.trim().is_empty() {
                docs.push(Document {
                    meta,
                    body,
                    source_path: file_path.to_string_lossy().into_owned(),
                });
            }
        }
    }

    Ok(docs)
}

/// Check if a file is a draft/unpublished across common static CMS conventions.
pub fn is_draft(content: &str) -> bool {
    let draft_re = Regex::new(r"(?m)^draft\s*[:=]\s*true\s*$").unwrap();
    let unpublished_re = Regex::new(r"(?m)^published\s*[:=]\s*false\s*$").unwrap();
    draft_re.is_match(content) || unpublished_re.is_match(content)
}

/// Strip markdown formatting, keeping readable text.
pub fn strip_markdown(content: &str) -> String {
    let mut result = content.to_string();

    let heading_re = Regex::new(r"(?m)^#{1,6}\s+").unwrap();
    result = heading_re.replace_all(&result, "").into_owned();

    let img_re = Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap();
    result = img_re.replace_all(&result, "$1").into_owned();

    let link_re = Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap();
    result = link_re.replace_all(&result, "$1").into_owned();

    let code_block_re = Regex::new(r"(?s)```.*?```").unwrap();
    result = code_block_re.replace_all(&result, "").into_owned();

    let html_re = Regex::new(r"<[^>]+>").unwrap();
    result = html_re.replace_all(&result, "").into_owned();

    let bq_re = Regex::new(r"(?m)^>\s*").unwrap();
    result = bq_re.replace_all(&result, "").into_owned();

    let hr_re = Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap();
    result = hr_re.replace_all(&result, "").into_owned();

    let blank_re = Regex::new(r"\n{3,}").unwrap();
    result = blank_re.replace_all(&result, "\n\n").into_owned();

    result.trim().to_string()
}

/// Parse TOML frontmatter delimited by `+++`.
pub fn parse_toml_frontmatter(content: &str, file_path: &Path) -> Result<(toml::Table, String)> {
    let rest = &content[3..];
    let end = rest
        .find("\n+++")
        .with_context(|| format!("no closing +++ in {}", file_path.display()))?;
    let toml_str = &rest[..end];
    let body = &rest[end + 4..];

    let table: toml::Table = toml::from_str(toml_str)
        .with_context(|| format!("parsing TOML in {}", file_path.display()))?;

    Ok((table, body.to_string()))
}

/// Parse YAML frontmatter delimited by `---`.
pub fn parse_yaml_frontmatter(content: &str, file_path: &Path) -> Result<(String, String)> {
    let rest = &content[3..];
    let end = rest
        .find("\n---")
        .with_context(|| format!("no closing --- in {}", file_path.display()))?;
    let yaml_str = rest[..end].to_string();
    let body = rest[end + 4..].to_string();
    Ok((yaml_str, body))
}

/// Extract a value from raw YAML text by key.
pub fn yaml_extract(yaml_str: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?m)^{key}\s*:\s*"?([^"\n]+)"?\s*$"#)).unwrap();
    re.captures(yaml_str).map(|c| c[1].trim().to_string())
}

/// Extract a list-like value from YAML frontmatter.
pub fn yaml_extract_list(yaml_str: &str, key: &str) -> Vec<String> {
    if let Some(raw) = yaml_extract(yaml_str, key) {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return trimmed[1..trimmed.len() - 1]
                .split(',')
                .map(|v| v.trim().trim_matches('"').trim_matches('\''))
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
        if !trimmed.is_empty() {
            return vec![trimmed.to_string()];
        }
    }
    Vec::new()
}

/// Build URL path from content-relative file path.
pub fn derive_url(file_path: &Path, content_root: &Path, index_file_names: &[&str]) -> String {
    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = relative.parent().unwrap_or(Path::new(""));

    let path = if index_file_names.iter().any(|name| *name == file_name) {
        format!("/{}", parent.to_string_lossy())
    } else {
        let stem = relative.with_extension("");
        format!("/{}", stem.to_string_lossy())
    };

    let mut url = path.replace("//", "/");
    if !url.starts_with('/') {
        url.insert(0, '/');
    }
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

/// Build metadata from basic fields.
pub fn meta(
    title: String,
    url: String,
    description: Option<String>,
    tags: Vec<String>,
    date: Option<String>,
) -> DocumentMeta {
    DocumentMeta {
        title,
        url,
        description,
        tags,
        date,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown_links() {
        let input = "Check [this link](https://example.com) out.";
        let result = strip_markdown(input);
        assert_eq!(result, "Check this link out.");
    }

    #[test]
    fn test_strip_markdown_images() {
        let input = "Look: ![alt text](image.png) here.";
        let result = strip_markdown(input);
        assert_eq!(result, "Look: alt text here.");
    }

    #[test]
    fn test_url_derivation_index() {
        let url = derive_url(
            Path::new("content/about/index.md"),
            Path::new("content"),
            &["index.md"],
        );
        assert_eq!(url, "/about/");
    }
}
