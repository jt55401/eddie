// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, is_frontmatter_draft, meta, parse_frontmatter_lines, parse_yaml_frontmatter,
    strip_bom, strip_markdown, yaml_extract, yaml_extract_list,
};

const DEFAULT_ROUTE_BASE_PATH: &str = "/docs";

static NUMERIC_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[-_]+").unwrap());
static BLOG_DATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})-(.+)$").unwrap());

/// Parser for Docusaurus docs/blog markdown. Docs URLs are rooted under
/// `/docs` (Docusaurus's `routeBasePath` default), numeric ordering prefixes
/// (`01-intro.md`, `02-guides/`) are stripped, `id`/`slug` frontmatter is
/// honored, and blog posts route by date rather than by file path.
pub struct DocusaurusParser;

impl ContentParser for DocusaurusParser {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        let content = strip_bom(content);
        let Some((doc_meta, body)) = parse_frontmatter(content, file_path, content_root)? else {
            return Ok(None);
        };
        let body = strip_markdown(&body);
        Ok(Some((doc_meta, body)))
    }

    fn extensions(&self) -> &[&str] {
        &["md", "markdown", "mdx"]
    }
}

fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
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
        let url = docusaurus_derive_url(
            file_path,
            content_root,
            fm.get("id"),
            fm.get("slug"),
            DEFAULT_ROUTE_BASE_PATH,
        );

        Ok(Some((meta(title, url, description, tags, date), body)))
    } else {
        let title = fallback_title(file_path);
        let url =
            docusaurus_derive_url(file_path, content_root, None, None, DEFAULT_ROUTE_BASE_PATH);
        Ok(Some((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        )))
    }
}

fn docusaurus_derive_url(
    file_path: &Path,
    content_root: &Path,
    id: Option<&str>,
    slug: Option<&str>,
    route_base_path: &str,
) -> String {
    if let Some(s) = slug.map(str::trim).filter(|s| !s.is_empty()) {
        let mut s = s.to_string();
        if !s.starts_with('/') {
            s.insert(0, '/');
        }
        if !s.ends_with('/') {
            s.push('/');
        }
        return s;
    }

    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let parent_segments: Vec<String> = relative
        .parent()
        .map(|p| {
            p.components()
                .map(|c| strip_numeric_prefix(&c.as_os_str().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default();

    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_index =
        file_name.eq_ignore_ascii_case("index.md") || file_name.eq_ignore_ascii_case("index.mdx");
    let stem = relative
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Docusaurus blog posts route by date, not by directory nesting.
    if parent_segments.first().map(String::as_str) == Some("blog") {
        if let Some(caps) = BLOG_DATE_RE.captures(&stem) {
            return format!("/blog/{}/{}/{}/{}/", &caps[1], &caps[2], &caps[3], &caps[4]);
        }
        let mut segs = parent_segments;
        if !is_index {
            segs.push(strip_numeric_prefix(&stem));
        }
        return finish_url(&segs, "");
    }

    let mut segments = parent_segments;
    if let Some(doc_id) = id.map(str::trim).filter(|s| !s.is_empty()) {
        segments.push(doc_id.to_string());
    } else if !is_index {
        segments.push(strip_numeric_prefix(&stem));
    }

    finish_url(&segments, route_base_path)
}

fn finish_url(segments: &[String], route_base_path: &str) -> String {
    let base = route_base_path.trim_matches('/');
    let mut all: Vec<String> = Vec::new();
    if !base.is_empty() {
        all.push(base.to_string());
    }
    all.extend(segments.iter().filter(|s| !s.is_empty()).cloned());

    let mut url = format!("/{}", all.join("/"));
    url = url.replace("//", "/");
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

fn strip_numeric_prefix(segment: &str) -> String {
    NUMERIC_PREFIX_RE.replace(segment, "").into_owned()
}

fn fallback_title(file_path: &Path) -> String {
    file_path
        .file_stem()
        .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
        .unwrap_or_else(|| "untitled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docusaurus_slug_overrides_url() {
        let parser = DocusaurusParser;
        let content = "---\ntitle: Intro\nslug: /docs/start\n---\nHello";
        let (meta, _) = parser
            .parse_file(content, Path::new("docs/intro.md"), Path::new("docs"))
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/docs/start/");
    }

    #[test]
    fn docusaurus_strips_numeric_ordering_prefixes() {
        let parser = DocusaurusParser;
        let content = "Just content, no frontmatter.";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("docs/getting-started/01-intro.md"),
                Path::new("docs"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/docs/getting-started/intro/");
    }

    #[test]
    fn docusaurus_index_routes_to_directory() {
        let parser = DocusaurusParser;
        let content = "Just content.";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("docs/tutorial/index.md"),
                Path::new("docs"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/docs/tutorial/");
    }

    #[test]
    fn docusaurus_id_frontmatter_overrides_filename() {
        let parser = DocusaurusParser;
        let content = "---\ntitle: Page\nid: custom-id\n---\nHello";
        let (meta, _) = parser
            .parse_file(content, Path::new("docs/guides/page.md"), Path::new("docs"))
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/docs/guides/custom-id/");
    }

    #[test]
    fn docusaurus_blog_post_routes_by_date() {
        let parser = DocusaurusParser;
        let content = "Hello post.";
        let (meta, _) = parser
            .parse_file(
                content,
                Path::new("blog/2019-05-28-hola.md"),
                Path::new("."),
            )
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/blog/2019/05/28/hola/");
    }
}
