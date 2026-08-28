use eddie::chunk::{
    ChunkStrategy, Document, DocumentMeta, chunk_document, chunk_document_with_strategy,
};
use eddie::parse::{ContentParser, HugoParser};
use std::path::Path;

fn make_doc(body: &str) -> Document {
    Document {
        meta: DocumentMeta {
            title: "Doc".to_string(),
            url: "/doc/".to_string(),
            description: None,
            tags: vec![],
            date: None,
        },
        body: body.to_string(),
        source_path: "content/doc.md".to_string(),
    }
}

#[test]
fn chunking_tracks_section_and_chunk_index() {
    let body = "Intro text.\n\n## First\n\nAlpha beta gamma.\n\n## Second\n\nDelta epsilon zeta.";
    let chunks = chunk_document(&make_doc(body), 64, 0);

    assert!(chunks.len() >= 3);
    assert!(chunks[0].meta.section.is_none());
    assert_eq!(chunks[1].meta.section.as_deref(), Some("First"));
    assert_eq!(chunks[2].meta.section.as_deref(), Some("Second"));
    for (idx, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.meta.chunk_index, idx);
    }
}

/// Regression test for the critical bug where `strip_markdown` deleted `#`
/// heading markers before the chunker ever ran, so `ChunkStrategy::Heading`
/// never found a section boundary for real CMS-parsed content (only unit
/// tests that fed raw markdown directly to `chunk_document` passed). This
/// exercises the actual parse -> chunk pipeline end to end.
#[test]
fn parsed_document_preserves_heading_sections_through_the_real_pipeline() {
    let parser = HugoParser;
    let content = "---\ntitle: Guide\n---\n\nIntro paragraph.\n\n## Section One\n\nFirst section body.\n\n## Section Two\n\nSecond section body.\n";
    let (meta, body) = parser
        .parse_file(content, Path::new("content/guide.md"), Path::new("content"))
        .unwrap()
        .unwrap();
    let doc = Document {
        meta,
        body,
        source_path: "content/guide.md".to_string(),
    };

    let chunks = chunk_document_with_strategy(&doc, 256, 0, ChunkStrategy::Heading);

    assert!(
        chunks.len() >= 3,
        "expected at least 3 chunks, got {}",
        chunks.len()
    );
    assert_eq!(chunks[0].meta.section, None);
    assert_eq!(chunks[1].meta.section.as_deref(), Some("Section One"));
    assert_eq!(chunks[2].meta.section.as_deref(), Some("Section Two"));
}
