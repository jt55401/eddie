// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_draft, meta, parse_yaml_frontmatter, strip_markdown,
    yaml_extract, yaml_extract_list,
};

/// Parser for Astro content collections and markdown pages.
pub struct AstroParser;

impl ContentParser for AstroParser {
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
        let body = strip_mdx_noise(&body);
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
        let url = derive_url(file_path, content_root, &["index.md", "index.mdx"]);
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

fn strip_mdx_noise(content: &str) -> String {
    let mut result = content.to_string();
    let import_re = Regex::new(r"(?m)^import\s+.*?;\s*$").unwrap();
    result = import_re.replace_all(&result, "").into_owned();

    let export_re = Regex::new(r"(?m)^export\s+.*?;\s*$").unwrap();
    result = export_re.replace_all(&result, "").into_owned();

    let jsx_inline_re = Regex::new(r"\{[^\n{}]*\}").unwrap();
    result = jsx_inline_re.replace_all(&result, "").into_owned();

    result
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
    fn astro_parser_parses_mdx() {
        let parser = AstroParser;
        let content = "---\ntitle: \"Welcome\"\n---\nimport X from './x'\n# Hello";
        let parsed = parser
            .parse_file(
                content,
                Path::new("src/content/docs/index.mdx"),
                Path::new("src/content"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(parsed.0.title, "Welcome");
        assert!(parsed.1.contains("Hello"));
    }
}
