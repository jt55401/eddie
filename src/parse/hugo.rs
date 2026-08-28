// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, is_toml_draft, meta, parse_toml_frontmatter, parse_yaml_frontmatter, strip_bom,
    strip_markdown, toml_value_to_string, yaml_extract, yaml_extract_list,
};

/// Hugo `[permalinks]`-style URL patterns, keyed by top-level content section.
/// Supports the placeholders `:year :month :day :slug :title :section
/// :filename :sections`.
#[derive(Debug, Clone)]
pub struct HugoOptions {
    pub permalinks: Vec<(String, String)>,
    /// Whether to lowercase and urlize path segments the way Hugo does by
    /// default (`disablePathToLower = false`). Defaults to `true`.
    pub lowercase: bool,
}

impl Default for HugoOptions {
    fn default() -> Self {
        HugoOptions {
            permalinks: Vec::new(),
            lowercase: true,
        }
    }
}

/// Content parser for Hugo static sites, using default options (see
/// [`HugoParser::with_options`] for `[permalinks]` support).
pub struct HugoParser;

/// A [`HugoParser`] configured with non-default [`HugoOptions`].
pub struct HugoParserWithOptions {
    options: HugoOptions,
}

impl HugoParser {
    pub fn with_options(options: HugoOptions) -> HugoParserWithOptions {
        HugoParserWithOptions { options }
    }
}

impl ContentParser for HugoParser {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_hugo_file(content, file_path, content_root, &default_options())
    }
}

impl ContentParser for HugoParserWithOptions {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_hugo_file(content, file_path, content_root, &self.options)
    }
}

fn default_options() -> HugoOptions {
    HugoOptions::default()
}

fn parse_hugo_file(
    content: &str,
    file_path: &Path,
    content_root: &Path,
    options: &HugoOptions,
) -> Result<Option<(DocumentMeta, String)>> {
    let content = strip_bom(content);

    let Some((doc_meta, body)) = parse_frontmatter(content, file_path, content_root, options)?
    else {
        return Ok(None);
    };

    let body = strip_shortcodes(&body);
    let body = strip_markdown(&body);

    Ok(Some((doc_meta, body)))
}

fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
    options: &HugoOptions,
) -> Result<Option<(DocumentMeta, String)>> {
    if content.starts_with("+++") {
        let (table, body) = parse_toml_frontmatter(content, file_path)?;
        if is_toml_draft(&table) {
            return Ok(None);
        }

        let title = table
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = table
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let date = table.get("date").and_then(toml_value_to_string);
        let tags = table
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let url_override = table.get("url").and_then(|v| v.as_str());
        let slug = table.get("slug").and_then(|v| v.as_str());

        let url = hugo_derive_url(
            file_path,
            content_root,
            url_override,
            slug,
            date.as_deref(),
            &title,
            options,
        );
        Ok(Some((meta(title, url, description, tags, date), body)))
    } else if content.starts_with("---") {
        let (yaml_str, body) = parse_yaml_frontmatter(content, file_path)?;
        let fm = super::parse_frontmatter_lines(&yaml_str);
        if super::is_frontmatter_draft(&fm) {
            return Ok(None);
        }

        let title = yaml_extract(&yaml_str, "title").unwrap_or_default();
        let description = yaml_extract(&yaml_str, "description");
        let date = yaml_extract(&yaml_str, "date");
        let tags = yaml_extract_list(&yaml_str, "tags");
        let url = hugo_derive_url(
            file_path,
            content_root,
            fm.get("url"),
            fm.get("slug"),
            date.as_deref(),
            &title,
            options,
        );
        Ok(Some((meta(title, url, description, tags, date), body)))
    } else {
        let url = hugo_derive_url(file_path, content_root, None, None, None, "", options);
        let title = file_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Some((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        )))
    }
}

fn hugo_derive_url(
    file_path: &Path,
    content_root: &Path,
    url_override: Option<&str>,
    slug: Option<&str>,
    date: Option<&str>,
    title: &str,
    options: &HugoOptions,
) -> String {
    if let Some(u) = url_override.map(str::trim).filter(|s| !s.is_empty()) {
        return normalize_url(u);
    }

    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_index =
        file_name.eq_ignore_ascii_case("_index.md") || file_name.eq_ignore_ascii_case("index.md");
    let parent = relative.parent().unwrap_or(Path::new(""));
    let parent_components: Vec<String> = parent
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let section = parent_components.first().cloned();
    let stem = strip_lang_suffix(
        &relative
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );

    if let Some(sec) = &section
        && let Some(pattern) = options
            .permalinks
            .iter()
            .find(|(s, _)| s == sec)
            .map(|(_, p)| p.clone())
    {
        // Hugo's `:slug` placeholder falls back to the title, then the
        // filename stem, when no explicit `slug` frontmatter is set.
        let slug_value = slug
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| urlize(s, options.lowercase))
            .unwrap_or_else(|| {
                if title.trim().is_empty() {
                    urlize(&stem, options.lowercase)
                } else {
                    urlize(title, options.lowercase)
                }
            });
        return render_permalink(
            &pattern,
            date,
            &slug_value,
            title,
            &stem,
            &parent_components,
            options.lowercase,
        );
    }

    let mut segments = parent_components;
    if !is_index {
        segments.push(stem);
    }

    if let Some(s) = slug.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(last) = segments.last_mut() {
            *last = s.trim_matches('/').to_string();
        } else {
            segments.push(s.trim_matches('/').to_string());
        }
    }

    let segments: Vec<String> = segments
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| urlize(&s, options.lowercase))
        .collect();

    let mut url = format!("/{}", segments.join("/"));
    url = url.replace("//", "/");
    if !url.starts_with('/') {
        url.insert(0, '/');
    }
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

fn render_permalink(
    pattern: &str,
    date: Option<&str>,
    slug: &str,
    title: &str,
    filename: &str,
    parent_components: &[String],
    lowercase: bool,
) -> String {
    let (year, month, day) = parse_date_parts(date);
    let sections = parent_components.join("/");
    let section = parent_components.first().cloned().unwrap_or_default();

    let mut out = pattern.to_string();
    out = out.replace(":year", &year);
    out = out.replace(":month", &month);
    out = out.replace(":day", &day);
    out = out.replace(":slug", slug);
    out = out.replace(":title", &urlize(title, lowercase));
    out = out.replace(":filename", &urlize(filename, lowercase));
    // `:sections` must be substituted before `:section` (its own prefix).
    out = out.replace(":sections", &sections);
    out = out.replace(":section", &section);

    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    out
}

fn parse_date_parts(date: Option<&str>) -> (String, String, String) {
    if let Some(d) = date
        && d.len() >= 10
        && d.as_bytes()[4] == b'-'
        && d.as_bytes()[7] == b'-'
    {
        return (
            d[0..4].to_string(),
            d[5..7].to_string(),
            d[8..10].to_string(),
        );
    }
    ("0000".to_string(), "00".to_string(), "00".to_string())
}

/// Strip a trailing short language code from a Hugo multilingual filename
/// stem (`hello.en` -> `hello`).
fn strip_lang_suffix(stem: &str) -> String {
    if let Some(dot) = stem.rfind('.') {
        let suffix = &stem[dot + 1..];
        if (2..=5).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_alphabetic()) {
            return stem[..dot].to_string();
        }
    }
    stem.to_string()
}

/// Lowercase (unless disabled) and urlize a single path segment the way Hugo
/// does: spaces and other non-alphanumeric runs collapse to a single `-`.
fn urlize(segment: &str, lowercase: bool) -> String {
    let s = if lowercase {
        segment.to_lowercase()
    } else {
        segment.to_string()
    };
    let mut out = String::new();
    let mut last_was_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize_url(url: &str) -> String {
    let mut normalized = url.trim().to_string();
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

static RAWHTML_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{\{<\s*/?\s*rawhtml\s*>\}\}").unwrap());
static REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{\{<\s*ref\s+"[^"]*"\s*>\}\}"#).unwrap());
static CERTIMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\{\{<\s*certimage\s+[^>]*>\}\}"#).unwrap());
static MERMAID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\{\{<\s*mermaid\s*>\}\}.*?\{\{<\s*/mermaid\s*>\}\}").unwrap()
});
static CLOSING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{<\s*closing\s*>\}\}").unwrap());
static GENERIC_ANGLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{<\s*/?[^>]*>\}\}").unwrap());
// Hugo's markdown shortcodes (`{{% notice %}} ... {{% /notice %}}`). Each tag
// occurrence is matched and removed independently, which leaves any prose
// between a paired open/close tag untouched.
static GENERIC_PERCENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{%\s*/?[^%]*%\}\}").unwrap());

fn strip_shortcodes(content: &str) -> String {
    let mut result = content.to_string();

    result = RAWHTML_RE.replace_all(&result, "").into_owned();
    result = REF_RE.replace_all(&result, "").into_owned();
    result = CERTIMAGE_RE.replace_all(&result, "").into_owned();
    result = MERMAID_RE.replace_all(&result, "").into_owned();
    result = CLOSING_RE.replace_all(&result, "").into_owned();
    result = GENERIC_ANGLE_RE.replace_all(&result, "").into_owned();
    result = GENERIC_PERCENT_RE.replace_all(&result, "").into_owned();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> HugoOptions {
        default_options()
    }

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
        let (meta, body) = parse_frontmatter(content, file_path, root, &options())
            .unwrap()
            .unwrap();
        assert_eq!(meta.title, "Test Post");
        assert_eq!(meta.date.as_deref(), Some("2024-01-01"));
        assert_eq!(meta.description.as_deref(), Some("A test"));
        assert_eq!(meta.tags, vec!["rust", "test"]);
        assert!(body.contains("Hello world."));
    }

    #[test]
    fn test_parse_toml_frontmatter_unquoted_datetime() {
        let content = "+++\ntitle = \"P\"\ndate = 2024-01-01T10:00:00-06:00\n+++\n\nBody.";
        let (meta, _) = parse_frontmatter(
            content,
            Path::new("content/p.md"),
            Path::new("content"),
            &options(),
        )
        .unwrap()
        .unwrap();
        assert!(
            meta.date
                .as_deref()
                .unwrap_or_default()
                .starts_with("2024-01-01"),
            "unquoted TOML datetime should be captured, got {:?}",
            meta.date
        );
    }

    #[test]
    fn test_parse_yaml_frontmatter() {
        let content = "---\ntitle: \"Skills\"\ndescription: \"My skills\"\n---\n\nContent here.";
        let file_path = Path::new("content/skills/_index.md");
        let root = Path::new("content");
        let (meta, body) = parse_frontmatter(content, file_path, root, &options())
            .unwrap()
            .unwrap();
        assert_eq!(meta.title, "Skills");
        assert_eq!(meta.description.as_deref(), Some("My skills"));
        assert!(body.contains("Content here."));
    }

    #[test]
    fn test_strip_shortcodes_angle_form() {
        let input = "Before {{< rawhtml >}}<div>stuff</div>{{< /rawhtml >}} After";
        let result = strip_shortcodes(input);
        assert!(!result.contains("rawhtml"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_strip_shortcodes_percent_form_keeps_body() {
        let input = "{{% notice warning %}}\nCareful\n{{% /notice %}}";
        let result = strip_shortcodes(input);
        assert!(!result.contains("notice"));
        assert!(!result.contains("warning"));
        assert!(result.contains("Careful"));
    }

    #[test]
    fn strip_shortcodes_handles_many_adjacent_tags_quickly() {
        let mut input = String::new();
        for i in 0..200 {
            input.push_str(&format!("{{{{% tag{i} %}}}}text{i}{{{{% /tag{i} %}}}}"));
        }
        let start = std::time::Instant::now();
        let result = strip_shortcodes(&input);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "shortcode stripping took too long: {:?}",
            start.elapsed()
        );
        assert!(result.contains("text0"));
        assert!(result.contains("text199"));
        assert!(!result.contains("{{%"));
    }

    #[test]
    fn test_url_derivation_regular_file() {
        let url = hugo_derive_url(
            Path::new("content/posts/my-post.md"),
            Path::new("content"),
            None,
            None,
            None,
            "",
            &options(),
        );
        assert_eq!(url, "/posts/my-post/");
    }

    #[test]
    fn test_url_derivation_index() {
        let url = hugo_derive_url(
            Path::new("content/about/_index.md"),
            Path::new("content"),
            None,
            None,
            None,
            "",
            &options(),
        );
        assert_eq!(url, "/about/");
    }

    #[test]
    fn test_url_lowercases_and_urlizes_segments() {
        let url = hugo_derive_url(
            Path::new("content/Posts/My Post.md"),
            Path::new("content"),
            None,
            None,
            None,
            "",
            &options(),
        );
        assert_eq!(url, "/posts/my-post/");
    }

    #[test]
    fn test_url_strips_language_suffix() {
        let url = hugo_derive_url(
            Path::new("content/posts/hello.en.md"),
            Path::new("content"),
            None,
            None,
            None,
            "",
            &options(),
        );
        assert_eq!(url, "/posts/hello/");
    }

    #[test]
    fn test_url_honors_slug_and_absolute_url_frontmatter() {
        let with_slug = hugo_derive_url(
            Path::new("content/posts/original.md"),
            Path::new("content"),
            None,
            Some("launch"),
            None,
            "",
            &options(),
        );
        assert_eq!(with_slug, "/posts/launch/");

        let with_url = hugo_derive_url(
            Path::new("content/posts/original.md"),
            Path::new("content"),
            Some("/about"),
            None,
            None,
            "",
            &options(),
        );
        assert_eq!(with_url, "/about/");
    }

    #[test]
    fn test_permalinks_pattern() {
        let mut opts = options();
        opts.permalinks
            .push(("posts".to_string(), "/:year/:slug/".to_string()));
        let url = hugo_derive_url(
            Path::new("content/posts/launch-day.md"),
            Path::new("content"),
            None,
            None,
            Some("2026-03-01T00:00:00Z"),
            "",
            &opts,
        );
        assert_eq!(url, "/2026/launch-day/");
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

    #[test]
    fn test_hugo_parser_skips_draft_marker_only_in_frontmatter() {
        // A body that merely mentions "draft: true" (e.g. a code sample)
        // must not be mistaken for an actual draft flag.
        let parser = HugoParser;
        let content = "+++\ntitle = \"Tutorial\"\n+++\n\n```\ndraft: true\n```\nBody.";
        let result = parser
            .parse_file(content, Path::new("content/post.md"), Path::new("content"))
            .unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_hugo_parser_skips_expired_page() {
        let parser = HugoParser;
        let content = "+++\ntitle = \"Old\"\nexpiryDate = 2000-01-01T00:00:00Z\n+++\nBody.";
        let result = parser
            .parse_file(content, Path::new("content/post.md"), Path::new("content"))
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_hugo_parser_strips_bom() {
        let parser = HugoParser;
        let content = "\u{FEFF}---\ntitle: Bom\n---\nBody";
        let (meta, body) = parser
            .parse_file(content, Path::new("content/post.md"), Path::new("content"))
            .unwrap()
            .unwrap();
        assert_eq!(meta.title, "Bom");
        assert!(body.contains("Body"));
    }

    #[test]
    fn test_with_options_permalinks_via_parser() {
        let mut opts = options();
        opts.permalinks
            .push(("posts".to_string(), "/:year/:slug/".to_string()));
        let parser = HugoParser::with_options(opts);
        let content = "+++\ntitle = \"Launch Day\"\ndate = 2026-03-01T00:00:00Z\n+++\nBody.";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("content/posts/launch.md"),
                Path::new("content"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/2026/launch-day/");
    }
}
