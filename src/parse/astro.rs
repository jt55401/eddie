// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_frontmatter_draft, meta, parse_frontmatter_lines,
    parse_yaml_frontmatter, strip_bom, strip_markdown, yaml_extract, yaml_extract_list,
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
        let content = strip_bom(content);
        let Some((doc_meta, body)) = parse_frontmatter(content, file_path, content_root)? else {
            return Ok(None);
        };
        let body = strip_mdx_noise(&body);
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
        let url = derive_url(file_path, content_root, &["index.md", "index.mdx"]);
        Ok(Some((meta(title, url, description, tags, date), body)))
    } else {
        let title = fallback_title(file_path);
        let url = derive_url(file_path, content_root, &["index.md", "index.mdx"]);
        Ok(Some((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        )))
    }
}

static IMPORT_EXPORT_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(import|export)\b").unwrap());
static STATEMENT_TERMINATES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(;\s*$)|(from\s*['"][^'"]*['"]\s*;?\s*$)"#).unwrap());

/// Strip MDX import/export statements (with or without a trailing semicolon,
/// single- or multi-line) and bare JSX expressions that occupy their own
/// line. Inline `{expr}` fragments embedded in prose are left alone so real
/// text around them survives.
fn strip_mdx_noise(content: &str) -> String {
    let mut out_lines: Vec<&str> = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        if IMPORT_EXPORT_START_RE.is_match(trimmed) {
            if !STATEMENT_TERMINATES_RE.is_match(trimmed) {
                for cont in lines.by_ref() {
                    if STATEMENT_TERMINATES_RE.is_match(cont.trim()) {
                        break;
                    }
                }
            }
            continue;
        }

        if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() > 1 {
            // A JSX expression that occupies its own line.
            continue;
        }

        out_lines.push(line);
    }

    out_lines.join("\n")
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
        assert!(!parsed.1.contains("import"));
    }

    #[test]
    fn astro_strips_multiline_import_without_semicolon() {
        let input = "import {\n  a,\n  b\n} from './x'\n\nText body.";
        let result = strip_mdx_noise(input);
        assert!(!result.contains("import"));
        assert!(result.contains("Text body."));
    }

    #[test]
    fn astro_strips_export_statement() {
        let input = "export const meta = { title: 'x' };\n\nBody text.";
        let result = strip_mdx_noise(input);
        assert!(!result.contains("export"));
        assert!(result.contains("Body text."));
    }

    #[test]
    fn astro_strips_own_line_jsx_expression_but_keeps_inline_prose() {
        let input = "{props.count}\n\nText {props.count} more prose here.";
        let result = strip_mdx_noise(input);
        assert!(!result.lines().any(|l| l.trim() == "{props.count}"));
        assert!(result.contains("Text {props.count} more prose here."));
    }
}
