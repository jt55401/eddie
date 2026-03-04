// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_draft, meta, parse_toml_frontmatter, parse_yaml_frontmatter,
    strip_markdown, yaml_extract, yaml_extract_list,
};

/// Content parser for Hugo static sites.
pub struct HugoParser;

impl ContentParser for HugoParser {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        if is_draft(content) {
            return Ok(None);
        }

        let (meta, body) = parse_frontmatter(content, file_path, content_root)?;
        let body = strip_shortcodes(&body);
        let body = strip_markdown(&body);

        Ok(Some((meta, body)))
    }
}

fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
) -> Result<(DocumentMeta, String)> {
    if content.starts_with("+++") {
        let (table, body) = parse_toml_frontmatter(content, file_path)?;
        let title = table
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = table
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let date = table
            .get("date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let tags = table
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let url = derive_url(file_path, content_root, &["_index.md", "index.md"]);
        Ok((meta(title, url, description, tags, date), body))
    } else if content.starts_with("---") {
        let (yaml_str, body) = parse_yaml_frontmatter(content, file_path)?;
        let title = yaml_extract(&yaml_str, "title").unwrap_or_default();
        let description = yaml_extract(&yaml_str, "description");
        let date = yaml_extract(&yaml_str, "date");
        let tags = yaml_extract_list(&yaml_str, "tags");
        let url = derive_url(file_path, content_root, &["_index.md", "index.md"]);
        Ok((meta(title, url, description, tags, date), body))
    } else {
        let url = derive_url(file_path, content_root, &["_index.md", "index.md"]);
        let title = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        ))
    }
}

fn strip_shortcodes(content: &str) -> String {
    let mut result = content.to_string();

    let rawhtml_re = Regex::new(r"(?s)\{\{<\s*/?\s*rawhtml\s*>\}\}").unwrap();
    result = rawhtml_re.replace_all(&result, "").into_owned();

    let ref_re = Regex::new(r#"\{\{<\s*ref\s+"[^"]*"\s*>\}\}"#).unwrap();
    result = ref_re.replace_all(&result, "").into_owned();

    let certimage_re = Regex::new(r#"\{\{<\s*certimage\s+[^>]*>\}\}"#).unwrap();
    result = certimage_re.replace_all(&result, "").into_owned();

    let mermaid_re = Regex::new(r"(?s)\{\{<\s*mermaid\s*>\}\}.*?\{\{<\s*/mermaid\s*>\}\}").unwrap();
    result = mermaid_re.replace_all(&result, "").into_owned();

    let closing_re = Regex::new(r"\{\{<\s*closing\s*>\}\}").unwrap();
    result = closing_re.replace_all(&result, "").into_owned();

    let generic_re = Regex::new(r"\{\{<\s*/?[^>]*>\}\}").unwrap();
    result = generic_re.replace_all(&result, "").into_owned();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_toml_frontmatter() {
        let content = r#"+++
title = "Test Post"
date = "2024-01-01"
description = "A test"
tags = ["rust", "test"]
+++

Hello world."#;
        let file_path = Path::new("content/posts/test.md");
        let root = Path::new("content");
        let (meta, body) = parse_frontmatter(content, file_path, root).unwrap();
        assert_eq!(meta.title, "Test Post");
        assert_eq!(meta.date.as_deref(), Some("2024-01-01"));
        assert_eq!(meta.description.as_deref(), Some("A test"));
        assert_eq!(meta.tags, vec!["rust", "test"]);
        assert!(body.contains("Hello world."));
    }

    #[test]
    fn test_parse_yaml_frontmatter() {
        let content = "---\ntitle: \"Skills\"\ndescription: \"My skills\"\n---\n\nContent here.";
        let file_path = Path::new("content/skills/_index.md");
        let root = Path::new("content");
        let (meta, body) = parse_frontmatter(content, file_path, root).unwrap();
        assert_eq!(meta.title, "Skills");
        assert_eq!(meta.description.as_deref(), Some("My skills"));
        assert!(body.contains("Content here."));
    }

    #[test]
    fn test_strip_shortcodes() {
        let input = "Before {{< rawhtml >}}<div>stuff</div>{{< /rawhtml >}} After";
        let result = strip_shortcodes(input);
        assert!(!result.contains("rawhtml"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_url_derivation_regular_file() {
        let url = derive_url(
            Path::new("content/posts/my-post.md"),
            Path::new("content"),
            &["_index.md", "index.md"],
        );
        assert_eq!(url, "/posts/my-post/");
    }

    #[test]
    fn test_hugo_parser_skips_draft() {
        let parser = HugoParser;
        let content = "+++\ntitle = \"Draft\"\ndraft = true\n+++\nBody text.";
        let result = parser
            .parse_file(content, Path::new("content/post.md"), Path::new("content"))
            .unwrap();
        assert!(result.is_none());
    }
}
