use eddie::chunk::ChunkMeta;
use eddie::index::{DenseLane, IndexBuilder, SCOPE_CHUNKS, SearchIndex};
use eddie::manifest::{DenseSpec, Family, Pooling, Quant, RuntimeSpec};

fn spec() -> DenseSpec {
    DenseSpec {
        id: "minilm".to_string(),
        model: "sentence-transformers/multi-qa-MiniLM-L6-cos-v1".to_string(),
        family: Family::Bert,
        dim: 3,
        pooling: Pooling::Mean,
        normalize: true,
        query_prefix: String::new(),
        doc_prefix: String::new(),
        max_seq_len: 512,
        revision: None,
        quant: Quant::Int8,
        runtime: RuntimeSpec::WasmCandle {
            files: vec![
                "config.json".into(),
                "tokenizer.json".into(),
                "model.safetensors".into(),
            ],
            base_url: None,
            bytes: None,
        },
    }
}

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
    let mut builder = IndexBuilder::new();
    builder
        .add_chunks(metadata, vec!["hello world".to_string()], vec![0])
        .unwrap();
    builder
        .add_dense_lane(
            SCOPE_CHUNKS,
            DenseLane::from_f32(spec(), 3, 1, &[0.1, 0.2, 0.3], Quant::Int8).unwrap(),
        )
        .unwrap();
    let index = builder.finish().unwrap();

    let mut out = Vec::new();
    index.write_ed_to(&mut out).unwrap();

    let manifest = SearchIndex::manifest_from_bytes(&out).unwrap();
    assert_eq!(manifest.format, 5);
    assert_eq!(manifest.dense[0].id, "minilm");
    assert_eq!(manifest.dense[0].dim, 3);

    let restored = SearchIndex::from_bytes(&out).unwrap();
    assert_eq!(restored.texts, vec!["hello world"]);
    assert_eq!(restored.dense.len(), 1);
    assert_eq!(restored.dense[0].rows, 1);
    assert_eq!(restored.bm25.num_docs, 1);
    assert!(restored.sparse.is_none());
}
