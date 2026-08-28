// SPDX-License-Identifier: GPL-3.0-only

use std::path::Path;

use anyhow::Result;

use crate::chunk::DocumentMeta;

use super::{
    ContentParser, derive_url, is_frontmatter_draft, meta, parse_frontmatter_lines,
    parse_yaml_frontmatter, strip_bom, strip_markdown, yaml_extract, yaml_extract_list,
};

/// Parser for MkDocs markdown docs.
pub struct MkDocsParser;

impl ContentParser for MkDocsParser {
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
        let url = derive_url(
            file_path,
            content_root,
            &["index.md", "README.md", "readme.md"],
        );
        Ok(Some((meta(title, url, description, tags, date), body)))
    } else {
        let title = fallback_title(file_path);
        let url = derive_url(
            file_path,
            content_root,
            &["index.md", "README.md", "readme.md"],
        );
        Ok(Some((
            meta(title, url, None, Vec::new(), None),
            content.to_string(),
        )))
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
    fn mkdocs_index_url() {
        let parser = MkDocsParser;
        let content = "# Home";
        let (meta, _) = parser
            .parse_file(content, Path::new("docs/index.md"), Path::new("docs"))
            .unwrap()
            .unwrap();
        assert_eq!(meta.url, "/");
    }

    #[test]
    fn mkdocs_skips_draft_marker_only_in_frontmatter() {
        let parser = MkDocsParser;
        let content = "---\ntitle: Tutorial\n---\n\n```\ndraft: true\n```\nBody.";
        let result = parser
            .parse_file(content, Path::new("docs/tutorial.md"), Path::new("docs"))
            .unwrap();
        assert!(result.is_some());
    }
}
