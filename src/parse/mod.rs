// SPDX-License-Identifier: GPL-3.0-only

//! Content parsing for static-site CMSes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use walkdir::WalkDir;

use crate::chunk::{Document, DocumentMeta};

mod astro;
mod docusaurus;
mod eleventy;
mod html;
mod hugo;
mod jekyll;
mod mkdocs;

pub use astro::AstroParser;
pub use docusaurus::DocusaurusParser;
pub use eleventy::EleventyParser;
pub use html::{HtmlOptions, HtmlParser, HtmlParserWithOptions};
pub use hugo::{HugoOptions, HugoParser, HugoParserWithOptions};
pub use jekyll::{JekyllOptions, JekyllParser, JekyllParserWithOptions};
pub use mkdocs::MkDocsParser;

/// Trait for CMS-specific content parsing.
pub trait ContentParser {
    /// Parse a file's raw content into metadata and a cleaned body.
    /// Returns `Ok(None)` if the file should be skipped (draft, empty, etc.).
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>>;

    /// File extensions this parser handles.
    fn extensions(&self) -> &[&str] {
        &["md", "markdown"]
    }

    /// Whether a directory (identified by its bare name, not full path)
    /// should be skipped entirely during the content walk — e.g. Jekyll's
    /// `_drafts`/`_site`. The default skips dotfiles/dirs and common vendor
    /// directories that are never real content.
    fn should_skip_dir(&self, dir_name: &str) -> bool {
        dir_name.starts_with('.') || dir_name == "node_modules" || dir_name == "vendor"
    }
}

/// The outcome of walking a content directory: the documents that parsed
/// successfully, plus every entry that was skipped along the way (directory
/// walk errors, unreadable files, and parse failures), each with a reason.
pub struct ParseReport {
    pub docs: Vec<Document>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// Walk a content directory using the given parser, returning all published
/// documents. A malformed or unreadable file is logged to stderr and skipped
/// rather than aborting the whole build; use [`parse_content_dir_report`] if
/// you want the skip list (and counts) directly, or pass `strict: true` there
/// to restore fail-fast behavior.
pub fn parse_content_dir(path: &Path, parser: &dyn ContentParser) -> Result<Vec<Document>> {
    Ok(parse_content_dir_report(path, parser, false)?.docs)
}

/// Like [`parse_content_dir`], but returns the full report (documents plus
/// skipped entries) and lets the caller choose fail-fast (`strict: true`)
/// behavior instead of skip-and-continue.
pub fn parse_content_dir_report(
    path: &Path,
    parser: &dyn ContentParser,
    strict: bool,
) -> Result<ParseReport> {
    let extensions = parser.extensions();
    let mut docs = Vec::new();
    let mut skipped: Vec<(PathBuf, String)> = Vec::new();

    let walker = WalkDir::new(path).into_iter().filter_entry(|e| {
        if e.depth() == 0 || !e.file_type().is_dir() {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        !parser.should_skip_dir(&name)
    });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                let path_hint = err
                    .path()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| path.to_path_buf());
                let msg = format!("walking directory entry: {err}");
                eprintln!("warning: skipping {}: {}", path_hint.display(), msg);
                if strict {
                    bail!("{}: {}", path_hint.display(), msg);
                }
                skipped.push((path_hint, msg));
                continue;
            }
        };

        let is_match = entry.path().extension().is_some_and(|ext| {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            extensions.iter().any(|x| *x == ext)
        });
        if !is_match {
            continue;
        }

        let file_path = entry.path();
        let raw_content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(err) => {
                let msg = format!("reading file: {err}");
                eprintln!("warning: skipping {}: {}", file_path.display(), msg);
                if strict {
                    return Err(err).with_context(|| format!("reading {}", file_path.display()));
                }
                skipped.push((file_path.to_path_buf(), msg));
                continue;
            }
        };
        let content = strip_bom(&raw_content);

        match parser.parse_file(content, file_path, path) {
            Ok(Some((meta, body))) => {
                if !body.trim().is_empty() {
                    docs.push(Document {
                        meta,
                        body,
                        source_path: file_path.to_string_lossy().into_owned(),
                    });
                }
            }
            Ok(None) => {}
            Err(err) => {
                let msg = format!("{err:#}");
                eprintln!("warning: skipping {}: {}", file_path.display(), msg);
                if strict {
                    return Err(err).context(format!("parsing {}", file_path.display()));
                }
                skipped.push((file_path.to_path_buf(), msg));
            }
        }
    }

    Ok(ParseReport { docs, skipped })
}

/// Strip a leading UTF-8 BOM, which otherwise defeats `starts_with("---")` /
/// `starts_with("+++")` frontmatter detection (common on Windows-edited files).
pub fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{FEFF}').unwrap_or(content)
}

/// Draft/expiry/publish check for TOML frontmatter (Hugo `+++` blocks).
pub fn is_toml_draft(table: &toml::Table) -> bool {
    if matches!(table.get("draft"), Some(toml::Value::Boolean(true))) {
        return true;
    }
    if matches!(table.get("published"), Some(toml::Value::Boolean(false))) {
        return true;
    }
    if let Some(v) = table.get("expiryDate")
        && let Some(date_str) = toml_value_to_string(v)
        && is_past_date(&date_str)
    {
        return true;
    }
    if let Some(v) = table.get("publishDate")
        && let Some(date_str) = toml_value_to_string(v)
        && is_future_date(&date_str)
    {
        return true;
    }
    false
}

/// Draft/expiry/publish check for line-based YAML-ish frontmatter.
pub fn is_frontmatter_draft(fm: &Frontmatter) -> bool {
    if fm.get_bool("draft") == Some(true) {
        return true;
    }
    if fm.get_bool("published") == Some(false) {
        return true;
    }
    if let Some(expiry) = fm.get("expiryDate")
        && is_past_date(expiry)
    {
        return true;
    }
    if let Some(publish) = fm.get("publishDate")
        && is_future_date(publish)
    {
        return true;
    }
    false
}

/// Convert a TOML value to a plain string, honoring both a quoted string
/// (`date = "2024-01-01"`) and Hugo's default unquoted TOML datetime
/// (`date = 2024-01-01T00:00:00Z`), which `Value::as_str()` alone misses.
pub fn toml_value_to_string(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Datetime(d) => Some(d.to_string()),
        _ => None,
    }
}

/// Parse a leading `YYYY-MM-DD` date prefix and report whether it is strictly
/// before today (UTC). Returns `false` for anything that doesn't parse —
/// callers should fail open (treat unparsable dates as "not expired") rather
/// than silently drop content over a formatting quirk.
fn is_past_date(s: &str) -> bool {
    match civil_days_from_prefix(s) {
        Some(days) => days < today_days(),
        None => false,
    }
}

/// Parse a leading `YYYY-MM-DD` date prefix and report whether it is strictly
/// after today (UTC).
fn is_future_date(s: &str) -> bool {
    match civil_days_from_prefix(s) {
        Some(days) => days > today_days(),
        None => false,
    }
}

fn today_days() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() / 86_400) as i64
}

fn civil_days_from_prefix(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 || &s[4..5] != "-" || &s[7..8] != "-" {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    Some(days_from_civil(y, m, d))
}

/// Howard Hinnant's civil-from-days algorithm (days since 1970-01-01).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// A parsed value from line-based frontmatter: either a plain scalar or a
/// list (either an inline `[a, b]` or a YAML block list under a bare key).
#[derive(Debug, Clone)]
enum FmValue {
    Scalar(String),
    List(Vec<String>),
}

/// A small, dependency-free line-based frontmatter parser used for the YAML
/// (`---`) frontmatter blocks emitted by Jekyll/Eleventy/Docusaurus/Astro/
/// MkDocs and Hugo. It is not a general YAML parser — it only understands the
/// flat `key: value` shapes real frontmatter actually uses — but unlike a
/// single regex it handles single/double-quoted values, empty values, and
/// block lists (`tags:` followed by `  - a` / `  - b` lines) correctly.
#[derive(Debug, Default)]
pub struct Frontmatter {
    values: HashMap<String, FmValue>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(FmValue::Scalar(s)) if !s.is_empty() => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_list(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(FmValue::List(items)) => items.clone(),
            Some(FmValue::Scalar(s)) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key)
            .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" => Some(true),
                "false" | "no" => Some(false),
                _ => None,
            })
    }
}

/// Parse a YAML-ish frontmatter block into a [`Frontmatter`] map.
pub fn parse_frontmatter_lines(yaml_str: &str) -> Frontmatter {
    let mut values: HashMap<String, FmValue> = HashMap::new();
    let lines: Vec<&str> = yaml_str.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_end();
        let content = trimmed.trim_start();

        // Bare list-item lines are only meaningful as a continuation of the
        // previous key, which we already consume below; skip stragglers.
        if content.starts_with("- ") || content == "-" {
            i += 1;
            continue;
        }

        let Some(colon) = content.find(':') else {
            i += 1;
            continue;
        };
        let key = content[..colon].trim();
        if key.is_empty() || key.contains(' ') || key.contains('"') || key.contains('\'') {
            i += 1;
            continue;
        }
        let raw_value = content[colon + 1..].trim();

        if raw_value.is_empty() {
            // Could be a YAML block list on the following, more-indented lines.
            let base_indent = line.len() - line.trim_start().len();
            let mut items = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j];
                let next_content = next.trim_start();
                let next_indent = next.len() - next_content.len();
                if next_content.starts_with("- ") && next_indent > base_indent {
                    items.push(unquote(next_content[2..].trim()));
                    j += 1;
                } else {
                    break;
                }
            }
            if !items.is_empty() {
                values.insert(key.to_string(), FmValue::List(items));
                i = j;
                continue;
            }
            values.insert(key.to_string(), FmValue::Scalar(String::new()));
        } else if raw_value.starts_with('[') && raw_value.ends_with(']') && raw_value.len() >= 2 {
            let items = raw_value[1..raw_value.len() - 1]
                .split(',')
                .map(|v| unquote(v.trim()))
                .filter(|v| !v.is_empty())
                .collect();
            values.insert(key.to_string(), FmValue::List(items));
        } else {
            values.insert(key.to_string(), FmValue::Scalar(unquote(raw_value)));
        }

        i += 1;
    }

    Frontmatter { values }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'')
        {
            return s[1..s.len() - 1].to_string();
        }
    }
    // Block scalar indicators (`|`, `>`) have no inline value worth keeping.
    if s == "|" || s == ">" || s == "|-" || s == ">-" {
        return String::new();
    }
    s.to_string()
}

/// Extract a scalar value from raw YAML-ish frontmatter text by key.
pub fn yaml_extract(yaml_str: &str, key: &str) -> Option<String> {
    parse_frontmatter_lines(yaml_str)
        .get(key)
        .map(str::to_string)
}

/// Extract a list-like value from YAML frontmatter (inline `[a, b]` or a
/// YAML block list).
pub fn yaml_extract_list(yaml_str: &str, key: &str) -> Vec<String> {
    parse_frontmatter_lines(yaml_str).get_list(key)
}

// The `regex` crate has no backreferences, so `<script>...</script>` and
// friends need one pattern per tag name rather than a single `\1` pattern.
static SCRIPT_STYLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:script|style|template)\b[^>]*>.*?</(?:script|style|template)\s*>")
        .unwrap()
});
static HTML_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static IMG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?```").unwrap());
static HTML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?[A-Za-z][^>]*>").unwrap());
static BLOCKQUOTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^>\s*").unwrap());
static HR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap());
static MULTI_BLANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
static SPACE_RUN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]{2,}").unwrap());
static SPACE_BEFORE_PUNCT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([.,!?;:])").unwrap());

/// Strip markdown/HTML formatting, keeping readable text.
///
/// ATX heading markers (`#`..`######`) are deliberately left in place: the
/// chunker (`crate::chunk`) needs them to find section boundaries, and folds
/// the heading text into the chunk body itself once it has consumed them.
pub fn strip_markdown(content: &str) -> String {
    let mut result = content.to_string();

    // Remove script/style/template bodies (and comments) before touching any
    // other tags, so their contents never leak into the indexed text.
    result = SCRIPT_STYLE_RE.replace_all(&result, "").into_owned();
    result = HTML_COMMENT_RE.replace_all(&result, "").into_owned();

    // Images are dropped entirely (no alt-text placeholder); links keep
    // their visible text and drop the URL. Images must be handled first
    // since `![alt](url)` also matches the link pattern.
    result = IMG_RE.replace_all(&result, "").into_owned();
    result = LINK_RE.replace_all(&result, "$1").into_owned();

    result = CODE_BLOCK_RE.replace_all(&result, "").into_owned();

    // Replace remaining tags with a space (not delete) so adjacent block
    // text doesn't fuse into one token, then collapse the resulting runs.
    result = HTML_TAG_RE.replace_all(&result, " ").into_owned();

    result = BLOCKQUOTE_RE.replace_all(&result, "").into_owned();
    result = HR_RE.replace_all(&result, "").into_owned();

    result = SPACE_RUN_RE.replace_all(&result, " ").into_owned();
    result = SPACE_BEFORE_PUNCT_RE
        .replace_all(&result, "$1")
        .into_owned();
    result = MULTI_BLANK_RE.replace_all(&result, "\n\n").into_owned();

    result.trim().to_string()
}

/// Parse TOML frontmatter delimited by `+++`.
pub fn parse_toml_frontmatter(content: &str, file_path: &Path) -> Result<(toml::Table, String)> {
    let rest = &content[3..];
    let end = rest
        .find("\n+++")
        .with_context(|| format!("no closing +++ in {}", file_path.display()))?;
    let toml_str = &rest[..end];
    let body = &rest[end + 4..];

    let table: toml::Table = toml::from_str(toml_str)
        .with_context(|| format!("parsing TOML in {}", file_path.display()))?;

    Ok((table, body.to_string()))
}

/// Parse YAML frontmatter delimited by `---`.
pub fn parse_yaml_frontmatter(content: &str, file_path: &Path) -> Result<(String, String)> {
    let rest = &content[3..];
    let end = rest
        .find("\n---")
        .with_context(|| format!("no closing --- in {}", file_path.display()))?;
    let yaml_str = rest[..end].to_string();
    let body = rest[end + 4..].to_string();
    Ok((yaml_str, body))
}

/// Build URL path from content-relative file path.
pub fn derive_url(file_path: &Path, content_root: &Path, index_file_names: &[&str]) -> String {
    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = relative.parent().unwrap_or(Path::new(""));

    let path = if index_file_names.iter().any(|name| *name == file_name) {
        format!("/{}", parent.to_string_lossy())
    } else {
        let stem = relative.with_extension("");
        format!("/{}", stem.to_string_lossy())
    };

    let mut url = path.replace("//", "/");
    if !url.starts_with('/') {
        url.insert(0, '/');
    }
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

/// Build metadata from basic fields.
pub fn meta(
    title: String,
    url: String,
    description: Option<String>,
    tags: Vec<String>,
    date: Option<String>,
) -> DocumentMeta {
    DocumentMeta {
        title,
        url,
        description,
        tags,
        date,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown_links() {
        let input = "Check [this link](https://example.com) out.";
        let result = strip_markdown(input);
        assert_eq!(result, "Check this link out.");
    }

    #[test]
    fn test_strip_markdown_images_dropped_entirely() {
        let input = "Look: ![alt text](image.png) here.";
        let result = strip_markdown(input);
        assert_eq!(result, "Look: here.");
    }

    #[test]
    fn test_strip_markdown_preserves_headings_for_chunking() {
        let input = "# Title\n\nUse [Eddie](https://example.com) to search docs.";
        let output = strip_markdown(input);
        assert!(
            output.contains('#'),
            "headings must survive strip_markdown so the chunker can find sections"
        );
        assert!(!output.contains("https://example.com"));
        assert!(output.contains("Title"));
        assert!(output.contains("Use Eddie to search docs."));
    }

    #[test]
    fn test_strip_markdown_removes_script_style_and_comments() {
        let input = "<p>One</p><p>Two</p>\n<script>var x = 1; alert('hi');</script>\n<style>.a{color:red}</style>\n<!-- note -->\nDone.";
        let output = strip_markdown(input);
        assert!(!output.contains("var x"));
        assert!(!output.contains("color:red"));
        assert!(!output.contains("note"));
        assert!(output.contains("One"));
        assert!(output.contains("Two"));
        assert!(output.contains("Done."));
    }

    #[test]
    fn test_strip_markdown_tags_become_whitespace() {
        let input = "<h2>Heading</h2><p>Hello <strong>world</strong>.</p>";
        let output = strip_markdown(input);
        assert_eq!(output, "Heading Hello world.");
    }

    #[test]
    fn test_url_derivation_index() {
        let url = derive_url(
            Path::new("content/about/index.md"),
            Path::new("content"),
            &["index.md"],
        );
        assert_eq!(url, "/about/");
    }

    #[test]
    fn test_strip_bom() {
        let content = "\u{FEFF}---\ntitle: Bom\n---\nBody";
        assert!(strip_bom(content).starts_with("---"));
        assert_eq!(strip_bom("no bom here"), "no bom here");
    }

    #[test]
    fn frontmatter_parses_quoted_scalars_and_block_lists() {
        let yaml =
            "title: 'Single quoted'\ndescription:\ndate: 2024-05-01\ntags:\n  - rust\n  - wasm";
        let fm = parse_frontmatter_lines(yaml);
        assert_eq!(fm.get("title"), Some("Single quoted"));
        assert_eq!(fm.get("description"), None);
        assert_eq!(fm.get("date"), Some("2024-05-01"));
        assert_eq!(fm.get_list("tags"), vec!["rust", "wasm"]);
    }

    #[test]
    fn frontmatter_parses_inline_list() {
        let yaml = "tags: [\"rust\", 'wasm', go]";
        let fm = parse_frontmatter_lines(yaml);
        assert_eq!(fm.get_list("tags"), vec!["rust", "wasm", "go"]);
    }

    #[test]
    fn frontmatter_double_quoted_title_has_no_stray_quotes() {
        let yaml = "title: \"Quoted Title\"";
        let fm = parse_frontmatter_lines(yaml);
        assert_eq!(fm.get("title"), Some("Quoted Title"));
    }

    #[test]
    fn is_frontmatter_draft_checks_expiry_and_publish_dates() {
        let past = parse_frontmatter_lines("expiryDate: 2000-01-01T00:00:00Z");
        assert!(is_frontmatter_draft(&past));

        let future = parse_frontmatter_lines("publishDate: 2999-01-01T00:00:00Z");
        assert!(is_frontmatter_draft(&future));

        let fine = parse_frontmatter_lines("title: fine");
        assert!(!is_frontmatter_draft(&fine));
    }

    #[test]
    fn is_toml_draft_honors_expiry_date() {
        let table: toml::Table =
            toml::from_str("title = \"X\"\nexpiryDate = 2000-01-01T00:00:00Z\n").unwrap();
        assert!(is_toml_draft(&table));

        let table: toml::Table = toml::from_str("title = \"X\"\n").unwrap();
        assert!(!is_toml_draft(&table));
    }

    #[test]
    fn toml_datetime_value_converts_to_string() {
        let table: toml::Table = toml::from_str("date = 2024-01-01T10:00:00-06:00\n").unwrap();
        let s = toml_value_to_string(table.get("date").unwrap()).unwrap();
        assert!(s.starts_with("2024-01-01"));
    }
}
