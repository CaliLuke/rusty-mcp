//! Mapping helpers for Qdrant payloads and chunk preparation.

use crate::{
    processing::{sanitize, types::SearchHit},
    qdrant::{self, compute_chunk_hash},
};
use serde_json::Value;
use std::collections::HashSet;

/// Chunk text with associated hash ready for ingestion.
#[derive(Debug, Clone)]
pub(crate) struct PreparedChunk {
    /// Chunk text content.
    pub(crate) text: String,
    /// Stable digest used for dedupe.
    pub(crate) chunk_hash: String,
}

/// Remove duplicate chunks within a document, keeping the first occurrence.
pub(crate) fn dedupe_chunks(chunks: Vec<String>) -> (Vec<PreparedChunk>, usize) {
    let mut seen = HashSet::new();
    let mut prepared = Vec::new();
    let mut skipped = 0;

    for text in chunks {
        if text.trim().is_empty() {
            continue;
        }
        let hash = compute_chunk_hash(&text);
        if seen.insert(hash.clone()) {
            prepared.push(PreparedChunk {
                text,
                chunk_hash: hash,
            });
        } else {
            skipped += 1;
        }
    }

    (prepared, skipped)
}

/// Map a Qdrant scored point into a user-friendly search hit structure.
pub(crate) fn map_scored_point(point: qdrant::ScoredPoint) -> SearchHit {
    let qdrant::ScoredPoint { id, score, payload } = point;

    let mut text = None;
    let mut project_id = None;
    let mut memory_type = None;
    let mut timestamp = None;
    let mut source_uri = None;
    let mut tags = None;

    if let Some(mut map) = payload {
        if let Some(Value::String(value)) = map.remove("text") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                text = Some(trimmed.to_string());
            }
        }
        if let Some(Value::String(value)) = map.remove("project_id") {
            project_id = sanitize::sanitize_project_id(Some(value));
        }
        if let Some(Value::String(value)) = map.remove("memory_type") {
            memory_type = sanitize::sanitize_memory_type(Some(value));
        }
        if let Some(Value::String(value)) = map.remove("timestamp") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                timestamp = Some(trimmed.to_string());
            }
        }
        if let Some(Value::String(value)) = map.remove("source_uri") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                source_uri = Some(trimmed.to_string());
            }
        }
        tags = sanitize::extract_tags(&map);
    }

    SearchHit {
        id,
        score,
        text,
        project_id,
        memory_type,
        tags,
        timestamp,
        source_uri,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};

    #[test]
    fn dedupe_chunks_removes_duplicates_and_counts_skips() {
        let chunks = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
        ];
        let (deduped, skipped) = dedupe_chunks(chunks);
        let texts: Vec<_> = deduped.iter().map(|chunk| chunk.text.as_str()).collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(skipped, 2);
        assert!(texts.contains(&"alpha"));
        assert!(texts.contains(&"beta"));
        assert_ne!(deduped[0].chunk_hash, deduped[1].chunk_hash);
    }

    #[test]
    fn map_scored_point_extracts_payload_fields() {
        let mut payload = Map::new();
        payload.insert("text".into(), Value::String("Example".into()));
        payload.insert("project_id".into(), Value::String("repo-a".into()));
        payload.insert("memory_type".into(), Value::String("semantic".into()));
        payload.insert(
            "timestamp".into(),
            Value::String("2025-01-01T00:00:00Z".into()),
        );
        payload.insert("source_uri".into(), Value::String("file://note".into()));
        payload.insert(
            "tags".into(),
            Value::Array(vec![
                Value::String("alpha".into()),
                Value::String("beta".into()),
            ]),
        );

        let point = qdrant::ScoredPoint {
            id: "memory-1".into(),
            score: 0.42,
            payload: Some(payload),
        };

        let hit: SearchHit = map_scored_point(point);
        assert_eq!(hit.id, "memory-1");
        assert!((hit.score - 0.42).abs() < f32::EPSILON);
        assert_eq!(hit.text.as_deref(), Some("Example"));
        assert_eq!(hit.project_id.as_deref(), Some("repo-a"));
        assert_eq!(hit.memory_type.as_deref(), Some("semantic"));
        assert_eq!(hit.timestamp.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(hit.source_uri.as_deref(), Some("file://note"));
        let tags = hit.tags.expect("tags present");
        assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);
    }
}
