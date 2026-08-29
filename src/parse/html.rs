// SPDX-License-Identifier: GPL-3.0-only

//! Content parser for rendered/static HTML output (`--cms html`).
//!
//! Unlike the other parsers in this module, this one does not read CMS
//! source files (markdown + frontmatter). It reads a site's *built* HTML
//! output (e.g. a Hugo `public/` directory, or any static-site generator's
//! render target) for sites whose copy lives in templates rather than
//! content files, so there is no frontmatter to fall back on: every piece of
//! metadata is pulled out of the rendered markup itself (`<meta>` tags,
//! `<h1>`, `<title>`, `<time datetime>`), and the body is reconstructed from
//! `<main>`/`<article>` (or `<body>` minus navigational chrome) by walking
//! the DOM and re-emitting the same "markdown-ish, headings preserved" text
//! the other parsers produce after [`super::strip_markdown`] — so the
//! chunker's heading-based section splitting works unchanged.

use std::path::Path;

use anyhow::{Context, Result};
use tl::{HTMLTag, Node, NodeHandle, Parser as TlParser, ParserOptions, VDom};
use unicode_segmentation::UnicodeSegmentation;

use crate::chunk::DocumentMeta;

use super::ContentParser;
use super::meta;

/// Minimum body word count for a page to be worth indexing. Pages under this
/// (typically empty shells, redirects, or near-empty stub pages) are skipped
/// the same way a Hugo draft is: `Ok(None)`, not an error, so they don't show
/// up in [`super::ParseReport::skipped`] — but a warning is still printed so
/// operators can grep/count how many were dropped.
const MIN_BODY_WORDS: usize = 20;

/// Tags whose entire subtree is dropped: never rendered, never recursed
/// into. Covers script/style/structural chrome that should never leak into
/// indexed text even when it sits inside `<main>`/`<article>`.
const EXCLUDED_TAGS: [&str; 8] = [
    "nav", "header", "footer", "aside", "script", "style", "noscript", "template",
];

/// Tags that start a new block: any inline text accumulated before them is
/// flushed as its own paragraph, and their own children are walked in a
/// fresh block context (so nested paragraphs/headings/lists inside them are
/// preserved as separate blocks rather than flattened into running prose).
const BLOCK_CONTAINER_TAGS: [&str; 20] = [
    "div",
    "p",
    "section",
    "article",
    "main",
    "blockquote",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "form",
    "fieldset",
    "details",
    "dl",
    "address",
    "figure",
    "td",
    "th",
    "caption",
];

/// Options for [`HtmlParser`]. See [`HtmlParser::with_options`].
#[derive(Debug, Clone, Copy, Default)]
pub struct HtmlOptions {
    /// Index pages whose `<meta name="robots">` contains `noindex` too.
    /// Off by default: a page the site itself asked search engines to skip
    /// is usually not meant to be found (thank-you pages, previews, etc.).
    pub include_noindex: bool,
}

/// Content parser for rendered HTML output, using default options (skips
/// `noindex` pages; see [`HtmlParser::with_options`] to include them).
pub struct HtmlParser;

/// A [`HtmlParser`] configured with non-default [`HtmlOptions`].
pub struct HtmlParserWithOptions {
    options: HtmlOptions,
}

impl HtmlParser {
    pub fn with_options(options: HtmlOptions) -> HtmlParserWithOptions {
        HtmlParserWithOptions { options }
    }
}

impl ContentParser for HtmlParser {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_html_file(content, file_path, content_root, &HtmlOptions::default())
    }

    fn extensions(&self) -> &[&str] {
        &["html", "htm"]
    }

    fn should_skip_dir(&self, dir_name: &str) -> bool {
        html_should_skip_dir(dir_name)
    }
}

impl ContentParser for HtmlParserWithOptions {
    fn parse_file(
        &self,
        content: &str,
        file_path: &Path,
        content_root: &Path,
    ) -> Result<Option<(DocumentMeta, String)>> {
        parse_html_file(content, file_path, content_root, &self.options)
    }

    fn extensions(&self) -> &[&str] {
        &["html", "htm"]
    }

    fn should_skip_dir(&self, dir_name: &str) -> bool {
        html_should_skip_dir(dir_name)
    }
}

/// Skip dotfiles/vendor dirs (the shared default) plus Hugo-style taxonomy
/// and pagination output: `tags/`, `categories/`, and `page/` (which holds
/// `page/2/`, `page/3/`, ... paginated list pages) are index-listing noise,
/// never real content.
fn html_should_skip_dir(dir_name: &str) -> bool {
    dir_name.starts_with('.')
        || dir_name == "node_modules"
        || dir_name == "vendor"
        || dir_name.eq_ignore_ascii_case("tags")
        || dir_name.eq_ignore_ascii_case("categories")
        || dir_name.eq_ignore_ascii_case("page")
}

fn parse_html_file(
    content: &str,
    file_path: &Path,
    content_root: &Path,
    options: &HtmlOptions,
) -> Result<Option<(DocumentMeta, String)>> {
    let file_name = file_path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if file_name == "404.html" || file_name == "404.htm" {
        return Ok(None);
    }

    let dom = tl::parse(content, ParserOptions::default())
        .with_context(|| format!("parsing HTML in {}", file_path.display()))?;
    let parser = dom.parser();

    if !options.include_noindex && is_noindex(&dom, parser) {
        return Ok(None);
    }

    let title = extract_title(&dom, parser);
    let description = find_meta(&dom, parser, "name", "description");
    let date = extract_date(&dom, parser);
    let url = derive_html_url(file_path, content_root);

    let root_children = find_content_root(&dom, parser);
    let mut blocks = Vec::new();
    render_block_children(&root_children, parser, &mut blocks);
    let body = blocks.join("\n\n");

    let words = body.unicode_words().count();
    if words < MIN_BODY_WORDS {
        eprintln!(
            "warning: skipping {} (thin body, {} word{})",
            file_path.display(),
            words,
            if words == 1 { "" } else { "s" }
        );
        return Ok(None);
    }

    Ok(Some((
        meta(title, url, description, Vec::new(), date),
        body,
    )))
}

/// Build a URL from a rendered-HTML file path the way a static-site server
/// would: an `index.html` (any case) becomes its parent directory with a
/// trailing slash; any other file keeps its own name and extension (a lone
/// `about.html` is served at `/about.html`, not `/about/`).
fn derive_html_url(file_path: &Path, content_root: &Path) -> String {
    let relative = file_path.strip_prefix(content_root).unwrap_or(file_path);
    let file_name = relative
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = relative.parent().unwrap_or(Path::new(""));
    let parent_str = parent.to_string_lossy().replace('\\', "/");

    let mut url = if file_name.eq_ignore_ascii_case("index.html")
        || file_name.eq_ignore_ascii_case("index.htm")
    {
        if parent_str.is_empty() {
            "/".to_string()
        } else {
            format!("/{parent_str}/")
        }
    } else if parent_str.is_empty() {
        format!("/{file_name}")
    } else {
        format!("/{parent_str}/{file_name}")
    };

    url = url.replace("//", "/");
    if !url.starts_with('/') {
        url.insert(0, '/');
    }
    url
}

fn is_noindex(dom: &VDom, parser: &TlParser) -> bool {
    find_meta(dom, parser, "name", "robots")
        .map(|content| content.to_ascii_lowercase().contains("noindex"))
        .unwrap_or(false)
}

/// Scan every `<meta>` tag for one whose `key_attr` (`name` or `property`)
/// equals `key_value` (case-insensitively), and return its decoded `content`.
fn find_meta(dom: &VDom, parser: &TlParser, key_attr: &str, key_value: &str) -> Option<String> {
    let iter = dom.query_selector("meta")?;
    for handle in iter {
        let Some(tag) = handle.get(parser).and_then(Node::as_tag) else {
            continue;
        };
        let matches = tag
            .attributes()
            .get(key_attr)
            .flatten()
            .map(|v| v.as_utf8_str().eq_ignore_ascii_case(key_value))
            .unwrap_or(false);
        if !matches {
            continue;
        }
        let content = tag.attributes().get("content").flatten()?;
        let text = decode_entities(content.as_utf8_str().trim());
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn first_tag(dom: &VDom, tag_name: &str) -> Option<NodeHandle> {
    dom.query_selector(tag_name)?.next()
}

fn extract_title(dom: &VDom, parser: &TlParser) -> String {
    if let Some(t) = find_meta(dom, parser, "property", "og:title") {
        return t;
    }
    if let Some(handle) = first_tag(dom, "h1")
        && let Some(tag) = handle.get(parser).and_then(Node::as_tag)
    {
        let text = finalize_text(&render_inline_children(
            tag.children().top().as_slice(),
            parser,
        ));
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(handle) = first_tag(dom, "title")
        && let Some(tag) = handle.get(parser).and_then(Node::as_tag)
    {
        let text = finalize_text(&render_inline_children(
            tag.children().top().as_slice(),
            parser,
        ));
        if !text.is_empty() {
            return strip_title_suffix(&text);
        }
    }
    String::new()
}

/// A bare `<title>` (no `og:title`, no `<h1>`) is often "Page Name | Site
/// Name" or "Page Name - Site Name"; strip a trailing separated segment so
/// the indexed title is the page's own, not the site's boilerplate suffix.
fn strip_title_suffix(title: &str) -> String {
    const SEPARATORS: [&str; 4] = [" | ", " — ", " – ", " - "];
    for sep in SEPARATORS {
        if let Some(idx) = title.rfind(sep) {
            let head = title[..idx].trim();
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    title.trim().to_string()
}

fn extract_date(dom: &VDom, parser: &TlParser) -> Option<String> {
    if let Some(d) = find_meta(dom, parser, "property", "article:published_time") {
        return Some(d);
    }
    let iter = dom.query_selector("time")?;
    for handle in iter {
        let Some(tag) = handle.get(parser).and_then(Node::as_tag) else {
            continue;
        };
        if let Some(dt) = tag.attributes().get("datetime").flatten() {
            let text = decode_entities(dt.as_utf8_str().trim());
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Find the DOM subtree to render as body text: `<main>` first, then
/// `<article>`, then `<body>` minus chrome (handled by [`is_excluded_tag`]
/// during the walk itself), falling back to the document's top-level nodes
/// for a bare fragment with no `<body>` wrapper (as in unit tests).
fn find_content_root(dom: &VDom, parser: &TlParser) -> Vec<NodeHandle> {
    for tag_name in ["main", "article", "body"] {
        if let Some(handle) = first_tag(dom, tag_name)
            && let Some(tag) = handle.get(parser).and_then(Node::as_tag)
        {
            return tag.children().top().as_slice().to_vec();
        }
    }
    dom.children().to_vec()
}

fn tag_name_lower(tag: &HTMLTag) -> String {
    tag.name().as_utf8_str().to_ascii_lowercase()
}

fn is_excluded_tag(tag: &HTMLTag, name: &str) -> bool {
    if EXCLUDED_TAGS.contains(&name) {
        return true;
    }
    let attrs = tag.attributes();
    if let Some(role) = attrs.get("role").flatten() {
        let role = role.as_utf8_str();
        if role.eq_ignore_ascii_case("navigation")
            || role.eq_ignore_ascii_case("banner")
            || role.eq_ignore_ascii_case("contentinfo")
        {
            return true;
        }
    }
    if let Some(hidden) = attrs.get("aria-hidden").flatten()
        && hidden.as_utf8_str().eq_ignore_ascii_case("true")
    {
        return true;
    }
    if let Some(id) = attrs.id()
        && id.as_utf8_str().to_ascii_lowercase().contains("eddie")
    {
        return true;
    }
    if let Some(classes) = attrs.class_iter()
        && classes
            .map(|c| c.to_ascii_lowercase())
            .any(|c| c.contains("eddie"))
    {
        return true;
    }
    false
}

/// Walk `children` in a block context, appending finished blocks (paragraphs,
/// headings, list groups, code blocks, ...) to `blocks` in document order.
fn render_block_children(children: &[NodeHandle], parser: &TlParser, blocks: &mut Vec<String>) {
    let mut para = String::new();

    for &handle in children {
        let Some(node) = handle.get(parser) else {
            continue;
        };
        match node {
            Node::Comment(_) => {}
            Node::Raw(bytes) => push_inline_text(&mut para, &decode_entities(&bytes.as_utf8_str())),
            Node::Tag(tag) => {
                let name = tag_name_lower(tag);
                if is_excluded_tag(tag, &name) {
                    continue;
                }
                let children_wrap = tag.children();
                let kids = children_wrap.top().as_slice();

                match name.as_str() {
                    "br" => para.push('\n'),
                    "hr" => flush_paragraph(&mut para, blocks),
                    "img" => {}
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        flush_paragraph(&mut para, blocks);
                        let level: usize = name[1..].parse().unwrap_or(1);
                        let text = finalize_text(&render_inline_children(kids, parser));
                        if !text.is_empty() {
                            blocks.push(format!("{} {}", "#".repeat(level.clamp(1, 6)), text));
                        }
                    }
                    "pre" => {
                        flush_paragraph(&mut para, blocks);
                        let code = render_pre_text(kids, parser);
                        let code = code.trim_matches('\n');
                        if !code.trim().is_empty() {
                            blocks.push(format!("```\n{code}\n```"));
                        }
                    }
                    "ul" | "ol" => {
                        flush_paragraph(&mut para, blocks);
                        let items = render_list_items(kids, parser);
                        if !items.is_empty() {
                            blocks.push(items.join("\n"));
                        }
                    }
                    "li" => {
                        // A stray <li> outside <ul>/<ol> (malformed markup);
                        // still worth keeping as its own bullet block.
                        flush_paragraph(&mut para, blocks);
                        let text = finalize_text(&render_inline_children(kids, parser));
                        if !text.is_empty() {
                            blocks.push(format!("* {text}"));
                        }
                    }
                    "code" => {
                        let text = render_inline_children(kids, parser);
                        push_inline_text(&mut para, &format!("`{}`", text.trim()));
                    }
                    _ if BLOCK_CONTAINER_TAGS.contains(&name.as_str()) => {
                        flush_paragraph(&mut para, blocks);
                        render_block_children(kids, parser, blocks);
                    }
                    _ => {
                        // Inline element (a, span, strong, em, time, ...):
                        // merge its text into the surrounding paragraph.
                        push_inline_text(&mut para, &render_inline_children(kids, parser));
                    }
                }
            }
        }
    }

    flush_paragraph(&mut para, blocks);
}

fn flush_paragraph(para: &mut String, blocks: &mut Vec<String>) {
    let text = finalize_text(para);
    if !text.is_empty() {
        blocks.push(text);
    }
    para.clear();
}

fn render_list_items(children: &[NodeHandle], parser: &TlParser) -> Vec<String> {
    let mut items = Vec::new();
    for &handle in children {
        let Some(tag) = handle.get(parser).and_then(Node::as_tag) else {
            continue;
        };
        if tag_name_lower(tag) != "li" {
            continue;
        }
        let text = finalize_text(&render_inline_children(
            tag.children().top().as_slice(),
            parser,
        ));
        if !text.is_empty() {
            items.push(format!("* {text}"));
        }
    }
    items
}

/// Render `children` as flattened inline text: headings/paragraphs/lists
/// inside an inline element don't create new blocks, they just contribute
/// their text to the current line.
fn render_inline_children(children: &[NodeHandle], parser: &TlParser) -> String {
    let mut buf = String::new();
    render_inline_into(children, parser, &mut buf);
    buf
}

fn render_inline_into(children: &[NodeHandle], parser: &TlParser, buf: &mut String) {
    for &handle in children {
        let Some(node) = handle.get(parser) else {
            continue;
        };
        match node {
            Node::Comment(_) => {}
            Node::Raw(bytes) => push_inline_text(buf, &decode_entities(&bytes.as_utf8_str())),
            Node::Tag(tag) => {
                let name = tag_name_lower(tag);
                if is_excluded_tag(tag, &name) {
                    continue;
                }
                match name.as_str() {
                    "br" => buf.push('\n'),
                    "img" => {}
                    "code" => {
                        let text = render_inline_children(tag.children().top().as_slice(), parser);
                        push_inline_text(buf, &format!("`{}`", text.trim()));
                    }
                    _ => render_inline_into(tag.children().top().as_slice(), parser, buf),
                }
            }
        }
    }
}

/// Collect the literal text content of a `<pre>` subtree: entities are
/// decoded, `<br>` becomes a real newline, but unlike [`push_inline_text`]
/// no whitespace is collapsed — code formatting/indentation is significant.
fn render_pre_text(children: &[NodeHandle], parser: &TlParser) -> String {
    let mut buf = String::new();
    collect_raw_text(children, parser, &mut buf);
    buf
}

fn collect_raw_text(children: &[NodeHandle], parser: &TlParser, buf: &mut String) {
    for &handle in children {
        let Some(node) = handle.get(parser) else {
            continue;
        };
        match node {
            Node::Comment(_) => {}
            Node::Raw(bytes) => buf.push_str(&decode_entities(&bytes.as_utf8_str())),
            Node::Tag(tag) => {
                let name = tag_name_lower(tag);
                if name == "br" {
                    buf.push('\n');
                    continue;
                }
                collect_raw_text(tag.children().top().as_slice(), parser, buf);
            }
        }
    }
}

/// Append `raw` to `buf`, collapsing internal whitespace runs to a single
/// space (HTML source formatting is not significant outside `<pre>`) and
/// inserting a boundary space if neither side already has one, so adjacent
/// inline elements never fuse into one word (`<b>Bold</b>ish` stays two
/// tokens, not "Boldish").
fn push_inline_text(buf: &mut String, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let mut normalized = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(ch);
            last_was_space = false;
        }
    }
    if normalized.is_empty() {
        return;
    }
    if !buf.is_empty()
        && !buf.ends_with(|c: char| c.is_whitespace())
        && !normalized.starts_with(' ')
    {
        buf.push(' ');
    }
    buf.push_str(&normalized);
}

/// Collapse each line's whitespace runs to single spaces and trim, while
/// preserving explicit `<br>`-inserted line breaks and dropping only
/// leading/trailing blank lines (an internal blank line from `<br><br>` is
/// kept — it reads as an intentional paragraph break within the block).
fn finalize_text(s: &str) -> String {
    let lines: Vec<String> = s
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let start = lines.iter().position(|l| !l.is_empty());
    let end = lines.iter().rposition(|l| !l.is_empty());
    match (start, end) {
        (Some(start), Some(end)) => lines[start..=end].join("\n"),
        _ => String::new(),
    }
}

/// Decode the small set of HTML entities that actually show up in rendered
/// site output: numeric (`&#39;`, `&#x27;`) and the common named ones.
/// Not a full HTML5 entity table (there are ~2000) — just enough to avoid
/// leaking `&amp;`/`&nbsp;`/typographic-quote entities into indexed text.
fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let bytes = input.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some(rel_end) = input[i..].find(';')
        {
            let end = i + rel_end;
            let entity = &input[i + 1..end];
            if let Some(decoded) = decode_entity_name(entity) {
                out.push(decoded);
                i = end + 1;
                continue;
            }
            if let Some(rest) = entity.strip_prefix('#') {
                let codepoint =
                    if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        rest.parse::<u32>().ok()
                    };
                if let Some(ch) = codepoint.and_then(char::from_u32) {
                    out.push(ch);
                    i = end + 1;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn decode_entity_name(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "copy" => '\u{00A9}',
        "reg" => '\u{00AE}',
        "trade" => '\u{2122}',
        "rsquo" => '\u{2019}',
        "lsquo" => '\u{2018}',
        "rdquo" => '\u{201D}',
        "ldquo" => '\u{201C}',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkStrategy, Document, chunk_document_with_strategy};

    fn parse(html: &str) -> (DocumentMeta, String) {
        HtmlParser
            .parse_file(
                html,
                Path::new("public/page/index.html"),
                Path::new("public"),
            )
            .unwrap()
            .expect("expected page to parse")
    }

    fn body_of(html: &str) -> String {
        parse(html).1
    }

    // --- URL derivation -----------------------------------------------

    #[test]
    fn url_index_html_strips_to_directory() {
        let url = derive_html_url(Path::new("public/about/index.html"), Path::new("public"));
        assert_eq!(url, "/about/");
    }

    #[test]
    fn url_root_index_html_is_slash() {
        let url = derive_html_url(Path::new("public/index.html"), Path::new("public"));
        assert_eq!(url, "/");
    }

    #[test]
    fn url_bare_html_file_keeps_extension() {
        let url = derive_html_url(Path::new("public/foo.html"), Path::new("public"));
        assert_eq!(url, "/foo.html");
    }

    #[test]
    fn url_nested_bare_html_file_keeps_extension() {
        let url = derive_html_url(Path::new("public/blog/foo.html"), Path::new("public"));
        assert_eq!(url, "/blog/foo.html");
    }

    // --- Title precedence ------------------------------------------------

    fn long_body() -> &'static str {
        "<p>one two three four five six seven eight nine ten eleven twelve \
         thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty \
         twentyone.</p>"
    }

    #[test]
    fn title_prefers_og_title_meta() {
        let html = format!(
            "<html><head><meta property=\"og:title\" content=\"OG Title\">\
             <title>Fallback | Site</title></head><body><h1>H1 Title</h1>{}</body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.title, "OG Title");
    }

    #[test]
    fn title_falls_back_to_first_h1() {
        let html = format!(
            "<html><head><title>Fallback | Site</title></head>\
             <body><h1>H1 Title</h1>{}</body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.title, "H1 Title");
    }

    #[test]
    fn title_falls_back_to_title_tag_and_strips_site_suffix() {
        let html = format!(
            "<html><head><title>Page Title | My Site</title></head>\
             <body>{}</body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.title, "Page Title");
    }

    #[test]
    fn title_tag_without_separator_is_kept_whole() {
        let html = format!(
            "<html><head><title>Just A Title</title></head><body>{}</body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.title, "Just A Title");
    }

    // --- noindex skipping --------------------------------------------------

    #[test]
    fn noindex_page_is_skipped_by_default() {
        let html = format!(
            "<html><head><meta name=\"robots\" content=\"noindex, nofollow\">\
             <title>Hidden</title></head><body>{}</body></html>",
            long_body()
        );
        let result = HtmlParser
            .parse_file(
                &html,
                Path::new("public/hidden/index.html"),
                Path::new("public"),
            )
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn noindex_page_is_kept_with_include_noindex_option() {
        let html = format!(
            "<html><head><meta name=\"robots\" content=\"noindex\">\
             <title>Hidden</title></head><body>{}</body></html>",
            long_body()
        );
        let parser = HtmlParser::with_options(HtmlOptions {
            include_noindex: true,
        });
        let result = parser
            .parse_file(
                &html,
                Path::new("public/hidden/index.html"),
                Path::new("public"),
            )
            .unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn page_404_is_always_skipped() {
        let html = format!("<html><body>{}</body></html>", long_body());
        let result = HtmlParser
            .parse_file(&html, Path::new("public/404.html"), Path::new("public"))
            .unwrap();
        assert!(result.is_none());
    }

    // --- main/article extraction vs body fallback --------------------------

    #[test]
    fn extracts_main_over_surrounding_body_chrome() {
        let html = format!(
            "<body><nav>Home About</nav><main>{}</main><footer>Copyright</footer></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("one two three"));
        assert!(!body.contains("Home About"));
        assert!(!body.contains("Copyright"));
    }

    #[test]
    fn extracts_article_when_no_main_present() {
        let html = format!(
            "<body><nav>Home About</nav><article>{}</article></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("one two three"));
        assert!(!body.contains("Home About"));
    }

    #[test]
    fn falls_back_to_body_minus_chrome_when_no_main_or_article() {
        let html = format!(
            "<body><nav>Home About</nav>{}<footer>Copyright</footer></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("one two three"));
        assert!(!body.contains("Home About"));
        assert!(!body.contains("Copyright"));
    }

    // --- nav/footer/widget removal ------------------------------------------

    #[test]
    fn removes_role_based_chrome() {
        let html = format!(
            "<body><div role=\"navigation\">Nav Links</div>\
             <main>{}</main><div role=\"contentinfo\">Footer Info</div></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(!body.contains("Nav Links"));
        assert!(!body.contains("Footer Info"));
    }

    #[test]
    fn removes_aria_hidden_elements() {
        let html = format!(
            "<body><main><span aria-hidden=\"true\">decor</span>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(!body.contains("decor"));
    }

    #[test]
    fn removes_eddie_widget_by_id_or_class() {
        let html = format!(
            "<body><main><div id=\"eddie-search\">Search widget</div>\
             <div class=\"eddie-panel\">Panel text</div>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(!body.contains("Search widget"));
        assert!(!body.contains("Panel text"));
    }

    // --- headings feed the chunker ------------------------------------------

    #[test]
    fn headings_become_atx_lines_and_chunk_into_sections() {
        let html = "<body><main><h2>Section One</h2><p>First section body with enough words \
             to pass the minimum threshold check for real for real for real.</p>\
             <h2>Section Two</h2><p>Second section body with enough words \
             to pass the minimum threshold check for real for real for real.</p></main></body>";
        let (meta, body) = parse(html);
        assert!(body.contains("## Section One"));
        assert!(body.contains("## Section Two"));

        let doc = Document {
            meta,
            body,
            source_path: "public/page/index.html".to_string(),
        };
        let chunks = chunk_document_with_strategy(&doc, 256, 0, ChunkStrategy::Heading);
        assert!(
            chunks.len() >= 2,
            "expected at least 2 chunks from 2 headed sections, got {}",
            chunks.len()
        );
    }

    // --- code block preservation --------------------------------------------

    #[test]
    fn code_block_is_preserved_as_fenced_block() {
        let html = format!(
            "<body><main>{}<pre><code>fn main() {{\n    println!(\"hi\");\n}}</code></pre></main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("```"));
        assert!(body.contains("fn main()"));
        assert!(body.contains("    println!(\"hi\");"));
    }

    #[test]
    fn inline_code_gets_backticks() {
        let html = format!(
            "<body><main><p>Run <code>cargo build</code> first.</p>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("`cargo build`"));
    }

    // --- entity decoding -----------------------------------------------------

    #[test]
    fn decodes_common_entities_in_body_text() {
        let html = format!(
            "<body><main><p>Fish &amp; chips &mdash; caf&eacute;? &nbsp; &lt;ok&gt;</p>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("Fish & chips"));
        assert!(body.contains('\u{2014}')); // mdash
        assert!(body.contains("<ok>"));
    }

    #[test]
    fn decodes_numeric_entities() {
        let html = format!(
            "<body><main><p>It&#39;s a &#x2013; dash test extra words here.</p>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("It's a"));
    }

    // --- lists -----------------------------------------------------------

    #[test]
    fn list_items_become_bullet_lines() {
        let html = format!(
            "<body><main><ul><li>Alpha</li><li>Beta</li><li>Gamma</li></ul>{}</main></body>",
            long_body()
        );
        let body = body_of(&html);
        assert!(body.contains("* Alpha"));
        assert!(body.contains("* Beta"));
        assert!(body.contains("* Gamma"));
    }

    // --- thin body dedupe --------------------------------------------------

    #[test]
    fn thin_body_page_is_skipped() {
        let html = "<html><body><main><p>Too short.</p></main></body></html>";
        let result = HtmlParser
            .parse_file(
                html,
                Path::new("public/thin/index.html"),
                Path::new("public"),
            )
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_body_page_is_skipped() {
        let html = "<html><body><main></main></body></html>";
        let result = HtmlParser
            .parse_file(
                html,
                Path::new("public/empty/index.html"),
                Path::new("public"),
            )
            .unwrap();
        assert!(result.is_none());
    }

    // --- date extraction -----------------------------------------------------

    #[test]
    fn date_prefers_article_published_time_meta() {
        let html = format!(
            "<html><head><meta property=\"article:published_time\" content=\"2026-01-15T00:00:00Z\">\
             </head><body><main><time datetime=\"2020-01-01\">Jan 1</time>{}</main></body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.date.as_deref(), Some("2026-01-15T00:00:00Z"));
    }

    #[test]
    fn date_falls_back_to_time_datetime() {
        let html = format!(
            "<body><main><time datetime=\"2020-01-01\">Jan 1</time>{}</main></body>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.date.as_deref(), Some("2020-01-01"));
    }

    // --- description ---------------------------------------------------------

    #[test]
    fn description_comes_from_meta_description() {
        let html = format!(
            "<html><head><meta name=\"description\" content=\"A helpful summary.\"></head>\
             <body><main>{}</main></body></html>",
            long_body()
        );
        let (meta, _) = parse(&html);
        assert_eq!(meta.description.as_deref(), Some("A helpful summary."));
    }

    // --- extensions / should_skip_dir ------------------------------------------

    #[test]
    fn extensions_include_html_and_htm() {
        let exts = HtmlParser.extensions();
        assert!(exts.contains(&"html"));
        assert!(exts.contains(&"htm"));
    }

    #[test]
    fn should_skip_dir_skips_taxonomy_and_pagination_dirs() {
        let parser = HtmlParser;
        assert!(parser.should_skip_dir("tags"));
        assert!(parser.should_skip_dir("categories"));
        assert!(parser.should_skip_dir("page"));
        assert!(parser.should_skip_dir(".git"));
        assert!(parser.should_skip_dir("node_modules"));
        assert!(!parser.should_skip_dir("blog"));
    }
}
