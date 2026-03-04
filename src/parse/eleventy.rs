// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_draft, meta, parse_yaml_frontmatter, strip_markdown,
    yaml_extract, yaml_extract_list,
};

/// Parser for Eleventy markdown content.
pub struct EleventyParser;

impl ContentParser for EleventyParser {
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
            .map(|s| {
                let mut permalink = s;
                if !permalink.starts_with('/') {
                    permalink.insert(0, '/');
                }
                if !permalink.ends_with('/') {
                    permalink.push('/');
                }
                permalink
            })
            .unwrap_or_else(|| {
                derive_url(
                    file_path,
                    content_root,
                    &["index.md", "README.md", "readme.md"],
                )
            });

        Ok((meta(title, url, description, tags, date), body))
    } else {
        let title = fallback_title(file_path);
        let url = derive_url(
            file_path,
            content_root,
            &["index.md", "README.md", "readme.md"],
        );
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
    fn eleventy_permalink_used() {
        let parser = EleventyParser;
        let content = "---\ntitle: About\npermalink: /about-us\n---\nAbout";
        let (meta, _) = parser
            .parse_file(content, Path::new("src/about.md"), Path::new("src"))
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/about-us/");
    }
}
