// SPDX-License-Identifier: GPL-3.0-only

//! Hugo content parsing: walk content directories, parse frontmatter, strip shortcodes.

use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use walkdir::WalkDir;

use crate::chunk::{Document, DocumentMeta};

/// Walk a content directory and parse all published markdown files into Documents.
pub fn parse_content_dir(path: &Path) -> Result<Vec<Document>> {
    let mut docs = Vec::new();

    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "md" || ext == "markdown")
        })
    {
        let file_path = entry.path();
        let content = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;

        if is_draft(&content) {
            continue;
        }

        let (meta, body) = parse_frontmatter(&content, file_path, path)?;
        let body = strip_shortcodes(&body);
        let body = strip_markdown(&body);

        if body.trim().is_empty() {
            continue;
        }

        docs.push(Document {
            meta,
            body,
            source_path: file_path.to_string_lossy().into_owned(),
        });
    }

    Ok(docs)
}

/// Check if a file is a draft/unpublished. Works generically across static site CMSes:
/// - Hugo: `draft = true` (TOML) / `draft: true` (YAML)
/// - Jekyll: `published: false`
/// - Eleventy/generic: `draft: true` or `published: false`
fn is_draft(content: &str) -> bool {
    let draft_re = Regex::new(r"(?m)^draft\s*[:=]\s*true\s*$").unwrap();
    let unpublished_re = Regex::new(r"(?m)^published\s*[:=]\s*false\s*$").unwrap();
    draft_re.is_match(content) || unpublished_re.is_match(content)
}

/// Parse frontmatter (TOML or YAML) and return metadata + body.
fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
) -> Result<(DocumentMeta, String)> {
    if content.starts_with("+++") {
        parse_toml_frontmatter(content, file_path, content_root)
    } else if content.starts_with("---") {
        parse_yaml_frontmatter(content, file_path, content_root)
    } else {
        // No frontmatter — use filename as title
        let url = derive_url(file_path, content_root);
        let title = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok((
            DocumentMeta {
                title,
                url,
                description: None,
                tags: Vec::new(),
                date: None,
            },
            content.to_string(),
        ))
    }
}

/// Parse TOML frontmatter delimited by `+++`.
fn parse_toml_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
) -> Result<(DocumentMeta, String)> {
    let rest = &content[3..]; // skip opening +++
    let end = rest
        .find("\n+++")
        .with_context(|| format!("no closing +++ in {}", file_path.display()))?;
    let toml_str = &rest[..end];
    let body = &rest[end + 4..]; // skip \n+++

    let table: toml::Table =
        toml::from_str(toml_str).with_context(|| format!("parsing TOML in {}", file_path.display()))?;

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

    let url = derive_url(file_path, content_root);

    Ok((
        DocumentMeta {
            title,
            url,
            description,
            tags,
            date,
        },
        body.to_string(),
    ))
}

/// Parse YAML frontmatter delimited by `---`. Simple regex-based extraction
/// since only a few files use this format.
fn parse_yaml_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
) -> Result<(DocumentMeta, String)> {
    let rest = &content[3..]; // skip opening ---
    let end = rest
        .find("\n---")
        .with_context(|| format!("no closing --- in {}", file_path.display()))?;
    let yaml_str = &rest[..end];
    let body = &rest[end + 4..]; // skip \n---

    let extract = |key: &str| -> Option<String> {
        let re = Regex::new(&format!(r#"(?m)^{key}\s*:\s*"?([^"\n]+)"?\s*$"#)).unwrap();
        re.captures(yaml_str).map(|c| c[1].trim().to_string())
    };

    let title = extract("title").unwrap_or_default();
    let description = extract("description");
    let date = extract("date");
    let url = derive_url(file_path, content_root);

    Ok((
        DocumentMeta {
            title,
            url,
            description,
            tags: Vec::new(),
            date,
        },
        body.to_string(),
    ))
}

/// Derive the URL path from a file path relative to the content root.
/// - `_index.md` → parent directory path
/// - `index.md` → parent directory path
/// - `foo.md` → `foo/`
fn derive_url(file_path: &Path, content_root: &Path) -> String {
    let relative = file_path
        .strip_prefix(content_root)
        .unwrap_or(file_path);

    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();

    let parent = relative.parent().unwrap_or(Path::new(""));

    let path_str = if file_name == "_index.md" || file_name == "index.md" {
        format!("/{}", parent.to_string_lossy())
    } else {
        let stem = relative.with_extension("");
        format!("/{}", stem.to_string_lossy())
    };

    // Normalize: ensure trailing slash, no double slashes
    let mut url = path_str.replace("//", "/");
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

/// Strip Hugo shortcodes from content.
pub fn strip_shortcodes(content: &str) -> String {
    let mut result = content.to_string();

    // {{< rawhtml >}}...{{< /rawhtml >}} — block shortcode with content
    let rawhtml_re = Regex::new(r"(?s)\{\{<\s*/?\s*rawhtml\s*>\}\}").unwrap();
    result = rawhtml_re.replace_all(&result, "").into_owned();

    // {{< ref "..." >}} — inline shortcode
    let ref_re = Regex::new(r#"\{\{<\s*ref\s+"[^"]*"\s*>\}\}"#).unwrap();
    result = ref_re.replace_all(&result, "").into_owned();

    // {{< certimage "..." "..." >}} — inline shortcode
    let certimage_re = Regex::new(r#"\{\{<\s*certimage\s+[^>]*>\}\}"#).unwrap();
    result = certimage_re.replace_all(&result, "").into_owned();

    // {{< mermaid >}}...{{< /mermaid >}} — block shortcode
    let mermaid_re = Regex::new(r"(?s)\{\{<\s*mermaid\s*>\}\}.*?\{\{<\s*/mermaid\s*>\}\}").unwrap();
    result = mermaid_re.replace_all(&result, "").into_owned();

    // {{< closing >}} — self-closing shortcode
    let closing_re = Regex::new(r"\{\{<\s*closing\s*>\}\}").unwrap();
    result = closing_re.replace_all(&result, "").into_owned();

    // Generic catch-all for any remaining shortcodes: {{< ... >}} and {{< /... >}}
    let generic_re = Regex::new(r"\{\{<\s*/?[^>]*>\}\}").unwrap();
    result = generic_re.replace_all(&result, "").into_owned();

    result
}

/// Strip markdown formatting, keeping readable text.
pub fn strip_markdown(content: &str) -> String {
    let mut result = content.to_string();

    // Remove heading markers (keep the text)
    let heading_re = Regex::new(r"(?m)^#{1,6}\s+").unwrap();
    result = heading_re.replace_all(&result, "").into_owned();

    // Remove images: ![alt](url) → alt
    let img_re = Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap();
    result = img_re.replace_all(&result, "$1").into_owned();

    // Remove links: [text](url) → text
    let link_re = Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap();
    result = link_re.replace_all(&result, "$1").into_owned();

    // Remove HTML tags
    let html_re = Regex::new(r"<[^>]+>").unwrap();
    result = html_re.replace_all(&result, "").into_owned();

    // Remove blockquote markers
    let bq_re = Regex::new(r"(?m)^>\s*").unwrap();
    result = bq_re.replace_all(&result, "").into_owned();

    // Remove horizontal rules
    let hr_re = Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap();
    result = hr_re.replace_all(&result, "").into_owned();

    // Collapse multiple blank lines
    let blank_re = Regex::new(r"\n{3,}").unwrap();
    result = blank_re.replace_all(&result, "\n\n").into_owned();

    result.trim().to_string()
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
    fn test_strip_rawhtml_shortcode() {
        let input = "Before {{< rawhtml >}}<div>stuff</div>{{< /rawhtml >}} After";
        let result = strip_shortcodes(input);
        assert!(!result.contains("rawhtml"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_strip_ref_shortcode() {
        let input = r#"See [link]({{< ref "other-page.md" >}}) for more."#;
        let result = strip_shortcodes(input);
        assert!(!result.contains("ref"));
        assert!(result.contains("See"));
    }

    #[test]
    fn test_strip_certimage_shortcode() {
        let input = r#"{{< certimage "images/cert.png" "My Certification">}}"#;
        let result = strip_shortcodes(input);
        assert!(result.trim().is_empty());
    }

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
    fn test_url_derivation_regular_file() {
        let url = derive_url(Path::new("content/posts/my-post.md"), Path::new("content"));
        assert_eq!(url, "/posts/my-post/");
    }

    #[test]
    fn test_url_derivation_index_md() {
        let url = derive_url(
            Path::new("content/about/index.md"),
            Path::new("content"),
        );
        assert_eq!(url, "/about/");
    }

    #[test]
    fn test_url_derivation_index_underscore() {
        let url = derive_url(
            Path::new("content/posts/_index.md"),
            Path::new("content"),
        );
        assert_eq!(url, "/posts/");
    }

    #[test]
    fn test_skip_draft() {
        let content = "+++\ntitle = \"Draft\"\ndraft = true\n+++\nBody.";
        assert!(is_draft(content));
    }

    #[test]
    fn test_skip_unpublished() {
        let content = "---\ntitle: Test\npublished: false\n---\nBody.";
        assert!(is_draft(content));
    }

    #[test]
    fn test_not_draft() {
        let content = "+++\ntitle = \"Published\"\ndraft = false\n+++\nBody.";
        assert!(!is_draft(content));
    }
}
