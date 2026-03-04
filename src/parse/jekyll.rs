// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_draft, meta, parse_yaml_frontmatter, strip_markdown,
    yaml_extract, yaml_extract_list,
};

/// Parser for Jekyll markdown content.
pub struct JekyllParser;

impl ContentParser for JekyllParser {
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
        let body = strip_markdown(&body);
        Ok(Some((meta, body)))
    }
}

fn parse_frontmatter(
    content: &str,
    file_path: &Path,
    content_root: &Path,
) -> Result<(DocumentMeta, String)> {
    if content.starts_with("---") {
        let (yaml_str, body) = parse_yaml_frontmatter(content, file_path)?;
        let title = yaml_extract(&yaml_str, "title").unwrap_or_else(|| fallback_title(file_path));
        let description = yaml_extract(&yaml_str, "description");
        let date = yaml_extract(&yaml_str, "date");
        let tags = yaml_extract_list(&yaml_str, "tags");
        let url = yaml_extract(&yaml_str, "permalink")
            .filter(|s| !s.is_empty())
            .map(normalize_url)
            .unwrap_or_else(|| derive_jekyll_url(file_path, content_root));
        Ok((meta(title, url, description, tags, date), body))
    } else {
        let title = fallback_title(file_path);
        let url = derive_jekyll_url(file_path, content_root);
        Ok((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        ))
    }
}

fn derive_jekyll_url(file_path: &Path, content_root: &Path) -> String {
    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let relative_str = relative.to_string_lossy();

    if relative_str.starts_with("_posts/") {
        let stem = relative
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let post_re = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})-(.+)$").unwrap();
        if let Some(caps) = post_re.captures(&stem) {
            let slug = caps.get(4).map(|m| m.as_str()).unwrap_or("post");
            return format!("/{}/{}/{}/{}/", &caps[1], &caps[2], &caps[3], slug);
        }
    }

    derive_url(
        file_path,
        content_root,
        &["index.md", "README.md", "readme.md"],
    )
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
        assert_eq!(meta.url, "/2026/01/15/my-first-post/");
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
}
