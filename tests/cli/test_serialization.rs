use std::io::{BufReader, BufWriter, Cursor};

use eddie::bm25::Bm25Index;
use eddie::chunk::ChunkMeta;
use eddie::index::SearchIndex;

#[test]
fn search_index_round_trip_preserves_chunk_texts() {
    let metadata = vec![ChunkMeta {
        title: "Doc".to_string(),
        url: "/doc/".to_string(),
        section: Some("Intro".to_string()),
        date: Some("2024-01-01".to_string()),
        granularity: Some("fine".to_string()),
        chunk_index: 0,
    }];
    let texts = vec!["hello world".to_string()];
    let index = SearchIndex::new(
        "test-model".to_string(),
        3,
        metadata,
        vec![0.1, 0.2, 0.3],
        Bm25Index::build(&["hello world"]),
        texts,
    );

    let mut out = Vec::new();
    index.write_to(BufWriter::new(&mut out)).unwrap();

    let restored = SearchIndex::read_from(BufReader::new(Cursor::new(out))).unwrap();
    assert_eq!(restored.model_id, "test-model");
    assert_eq!(restored.dim, 3);
    assert_eq!(restored.texts, vec!["hello world"]);
}
