// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, Frontmatter, derive_url, is_frontmatter_draft, meta, parse_frontmatter_lines,
    parse_yaml_frontmatter, strip_bom, strip_markdown, yaml_extract, yaml_extract_list,
};

static POST_DATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})-(.+)$").unwrap());

/// Jekyll permalink style: `"date"` (default, `/:categories/:year/:month/:day/:title.html`),
/// `"pretty"` (`/:categories/:year/:month/:day/:title/`), `"none"`
/// (`/:categories/:title.html`), or a custom pattern string using the same
/// placeholders.
#[derive(Debug, Clone)]
pub struct JekyllOptions {
    pub permalink: String,
}

impl Default for JekyllOptions {
    fn default() -> Self {
        JekyllOptions {
            permalink: "date".to_string(),
        }
    }
}

/// Parser for Jekyll markdown content, using the default `date` permalink
/// style (see [`JekyllParser::with_options`] for `pretty`/`none`/custom).
pub struct JekyllParser;

/// A [`JekyllParser`] configured with non-default [`JekyllOptions`].
pub struct JekyllParserWithOptions {
    options: JekyllOptions,
}

impl JekyllParser {
    pub fn with_options(options: JekyllOptions) -> JekyllParserWithOptions {
        JekyllParserWithOptions { options }
    }
}

impl ContentParser for JekyllParser {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_jekyll_file(content, file_path, content_root, &JekyllOptions::default())
    }

    fn should_skip_dir(&self, dir_name: &str) -> bool {
        jekyll_should_skip_dir(dir_name)
    }
}

impl ContentParser for JekyllParserWithOptions {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_jekyll_file(content, file_path, content_root, &self.options)
    }

    fn should_skip_dir(&self, dir_name: &str) -> bool {
        jekyll_should_skip_dir(dir_name)
    }
}

fn jekyll_should_skip_dir(dir_name: &str) -> bool {
    if dir_name.starts_with('.') || dir_name == "node_modules" || dir_name == "vendor" {
        return true;
    }
    if dir_name == "_site" || dir_name == "_drafts" {
        return true;
    }
    dir_name.starts_with('_') && dir_name != "_posts"
}

fn parse_jekyll_file(
    content: &str,
    file_path: &Path,
    content_root: &Path,
    options: &JekyllOptions,
) -> Result<Option<(DocumentMeta, String)>> {
    let content = strip_bom(content);

    let Some((doc_meta, body)) = parse_frontmatter(content, file_path, content_root, options)?
    else {
        return Ok(None);
    };
    let body = strip_markdown(&body);
    Ok(Some((doc_meta, body)))
}

fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
    options: &JekyllOptions,
) -> Result<Option<(DocumentMeta, String)>> {
    if content.starts_with("---") {
        let (yaml_str, body) = parse_yaml_frontmatter(content, file_path)?;
        let fm = parse_frontmatter_lines(&yaml_str);
        if is_frontmatter_draft(&fm) {
            return Ok(None);
        }

        let title = yaml_extract(&yaml_str, "title").unwrap_or_else(|| fallback_title(file_path));
        let description = yaml_extract(&yaml_str, "description");
        let date = yaml_extract(&yaml_str, "date");
        let tags = yaml_extract_list(&yaml_str, "tags");
        let url = yaml_extract(&yaml_str, "permalink")
            .filter(|s| !s.is_empty())
            .map(normalize_url)
            .unwrap_or_else(|| derive_jekyll_url(file_path, content_root, &fm, options));
        Ok(Some((meta(title, url, description, tags, date), body)))
    } else {
        let title = fallback_title(file_path);
        let url = derive_jekyll_url(file_path, content_root, &Frontmatter::default(), options);
        Ok(Some((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        )))
    }
}

fn derive_jekyll_url(
    file_path: &Path,
    content_root: &Path,
    fm: &Frontmatter,
    options: &JekyllOptions,
) -> String {
    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let components: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    if let Some(posts_idx) = components.iter().position(|c| c == "_posts") {
        let mut categories: Vec<String> = components[..posts_idx].to_vec();
        categories.extend(fm.get_list("categories"));

        let stem = relative
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(caps) = POST_DATE_RE.captures(&stem) {
            let slug = caps.get(4).map(|m| m.as_str()).unwrap_or("post");
            return jekyll_permalink_url(&caps[1], &caps[2], &caps[3], slug, &categories, options);
        }
    }

    derive_url(
        file_path,
        content_root,
        &["index.md", "README.md", "readme.md"],
    )
}

fn jekyll_permalink_pattern(style: &str) -> &str {
    match style {
        "pretty" => "/:categories/:year/:month/:day/:title/",
        "none" => "/:categories/:title.html",
        "date" => "/:categories/:year/:month/:day/:title.html",
        other => other,
    }
}

fn jekyll_permalink_url(
    year: &str,
    month: &str,
    day: &str,
    title: &str,
    categories: &[String],
    options: &JekyllOptions,
) -> String {
    let pattern = jekyll_permalink_pattern(&options.permalink);
    let cat_prefix = if categories.is_empty() {
        String::new()
    } else {
        format!("{}/", categories.join("/"))
    };

    let mut out = pattern.to_string();
    out = out.replace(":categories/", &cat_prefix);
    out = out.replace(":year", year);
    out = out.replace(":month", month);
    out = out.replace(":day", day);
    out = out.replace(":title", title);
    out = out.replace("//", "/");
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

fn fallback_title(file_path: &Path) -> String {
    file_path
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
        .unwrap_or_else(|| "untitled".to_string())
}

fn normalize_url(url: String) -> String {
    let mut normalized = url;
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jekyll_post_path_to_permalink() {
        let parser = JekyllParser;
        let content = "---\ntitle: Hello\n---\nBody";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("_posts/2026-01-15-my-first-post.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/2026/01/15/my-first-post.html");
    }

    #[test]
    fn jekyll_permalink_frontmatter_wins() {
        let parser = JekyllParser;
        let content = "---\ntitle: Hello\npermalink: /blog/hello\n---\nBody";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("_posts/2026-01-15-hello.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/blog/hello/");
    }

    #[test]
    fn jekyll_posts_anywhere_in_path_with_directory_categories() {
        let parser = JekyllParser;
        let content = "---\ntitle: Hello\n---\nBody";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("blog/_posts/2026-01-15-hi.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/blog/2026/01/15/hi.html");
    }

    #[test]
    fn jekyll_frontmatter_categories_are_included() {
        let parser = JekyllParser;
        let content = "---\ntitle: Hello\ncategories:\n  - tech\n  - rust\n---\nBody";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("_posts/2026-01-15-hi.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/tech/rust/2026/01/15/hi.html");
    }

    #[test]
    fn jekyll_pretty_permalink_style() {
        let opts = JekyllOptions {
            permalink: "pretty".to_string(),
        };
        let parser = JekyllParser::with_options(opts);
        let content = "---\ntitle: Hello\n---\nBody";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("_posts/2026-01-15-hi.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/2026/01/15/hi/");
    }

    #[test]
    fn jekyll_skips_drafts_site_and_vendor_dirs() {
        let parser = JekyllParser;
        assert!(parser.should_skip_dir("_drafts"));
        assert!(parser.should_skip_dir("_site"));
        assert!(parser.should_skip_dir("node_modules"));
        assert!(parser.should_skip_dir("vendor"));
        assert!(!parser.should_skip_dir("_posts"));
        assert!(!parser.should_skip_dir("posts"));
    }

    #[test]
    fn jekyll_skips_draft_marker_only_in_frontmatter() {
        let parser = JekyllParser;
        let content = "---\ntitle: Tutorial\n---\n\n```\ndraft: true\n```\nBody.";
        let result = parser
            .parse_file(
                content,
                Path::new("_posts/2026-01-15-hi.md"),
                Path::new("."),
            )
            .unwrap();
        assert!(result.is_some());
    }
}
