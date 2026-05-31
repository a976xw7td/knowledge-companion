//! Vector math and embedding blob tests. No API key needed.

use knowledge_companion::index::vector::{cosine_similarity, Embedding};

#[test]
fn test_cosine_identical() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![1.0f32, 0.0, 0.0];
    assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
}

#[test]
fn test_cosine_orthogonal() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);
}

#[test]
fn test_cosine_opposite() {
    let a = vec![1.0f32, 0.0];
    let b = vec![-1.0f32, 0.0];
    assert!((cosine_similarity(&a, &b) + 1.0).abs() < 0.001);
}

#[test]
fn test_embedding_blob_roundtrip() {
    let e = Embedding {
        model: "test".into(),
        dimensions: 4,
        vector: vec![0.1, -0.2, 0.3, -0.4],
    };
    let blob = e.to_blob();
    let d = Embedding::from_blob(&blob, "test", 4);
    assert_eq!(d.dimensions, 4);
    for (a, b) in e.vector.iter().zip(d.vector.iter()) {
        assert!((a - b).abs() < 0.001);
    }
}

#[test]
fn test_embedding_blob_zero() {
    let e = Embedding {
        model: "z".into(),
        dimensions: 3,
        vector: vec![0.0, 0.0, 0.0],
    };
    let blob = e.to_blob();
    let d = Embedding::from_blob(&blob, "z", 3);
    assert_eq!(d.vector, vec![0.0, 0.0, 0.0]);
}
