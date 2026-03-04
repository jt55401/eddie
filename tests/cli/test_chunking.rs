use eddie::chunk::{Document, DocumentMeta, chunk_document};

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
