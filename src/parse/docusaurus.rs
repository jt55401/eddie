// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_draft, meta, parse_yaml_frontmatter, strip_markdown,
    yaml_extract, yaml_extract_list,
};

/// Parser for Docusaurus docs/blog markdown.
pub struct DocusaurusParser;

impl ContentParser for DocusaurusParser {
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

    fn extensions(&self) -> &[&str] {
        &["md", "markdown", "mdx"]
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

        let url = yaml_extract(&yaml_str, "slug")
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut slug = s;
                if !slug.starts_with('/') {
                    slug.insert(0, '/');
                }
                if !slug.ends_with('/') {
                    slug.push('/');
                }
                slug
            })
            .unwrap_or_else(|| derive_url(file_path, content_root, &["index.md", "index.mdx"]));

        Ok((meta(title, url, description, tags, date), body))
    } else {
        let title = fallback_title(file_path);
        let url = derive_url(file_path, content_root, &["index.md", "index.mdx"]);
        Ok((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        ))
    }
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
}
