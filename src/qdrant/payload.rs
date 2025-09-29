//! Helpers for constructing and hashing Qdrant payloads.

use crate::qdrant::types::PayloadOverrides;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

/// Build the payload object stored alongside each indexed chunk.
pub(crate) fn build_payload(
    memory_id: &str,
    text: &str,
    timestamp_rfc3339: &str,
    chunk_hash: &str,
    overrides: &PayloadOverrides,
) -> Value {
    let mut payload = Map::new();
    payload.insert("memory_id".into(), Value::String(memory_id.to_string()));
    payload.insert(
        "project_id".into(),
        Value::String(
            overrides
                .project_id
                .clone()
                .unwrap_or_else(default_project_id),
        ),
    );
    payload.insert(
        "memory_type".into(),
        Value::String(
            overrides
                .memory_type
                .clone()
                .unwrap_or_else(default_memory_type),
        ),
    );
    payload.insert(
        "timestamp".into(),
        Value::String(timestamp_rfc3339.to_string()),
    );
    payload.insert("chunk_hash".into(), Value::String(chunk_hash.to_string()));
    payload.insert("text".into(), Value::String(text.to_string()));

    if let Some(source_uri) = overrides
        .source_uri
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        payload.insert("source_uri".into(), Value::String(source_uri.clone()));
    }

    if let Some(tags) = overrides.tags.as_ref().filter(|tags| !tags.is_empty()) {
        payload.insert(
            "tags".into(),
            Value::Array(tags.iter().map(|tag| Value::String(tag.clone())).collect()),
        );
    }

    if let Some(ids) = overrides
        .source_memory_ids
        .as_ref()
        .filter(|ids| !ids.is_empty())
    {
        payload.insert(
            "source_memory_ids".into(),
            Value::Array(ids.iter().map(|id| Value::String(id.clone())).collect()),
        );
    }

    if let Some(key) = overrides
        .summary_key
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        payload.insert("summary_key".into(), Value::String(key.clone()));
    }

    Value::Object(payload)
}

/// Compute a deterministic SHA-256 hash for the chunk text.
pub fn compute_chunk_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

/// Current timestamp formatted for payload storage.
pub(crate) fn current_timestamp_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn default_project_id() -> String {
    "default".to_string()
}

fn default_memory_type() -> String {
    "semantic".to_string()
}

/// Construct an identifier suitable for Qdrant payloads.
pub(crate) fn generate_memory_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_hash_is_stable() {
        let text = "Hello world";
        let h1 = compute_chunk_hash(text);
        let h2 = compute_chunk_hash(text);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn timestamp_is_rfc3339_like() {
        let ts = current_timestamp_rfc3339();
        assert!(ts.contains('T') && ts.ends_with('Z'));
    }

    #[test]
    fn payload_includes_defaults_and_text() {
        let id = generate_memory_id();
        let now = "2025-01-01T00:00:00Z";
        let chunk_hash = "abc123";
        let payload = build_payload(&id, "sample", now, chunk_hash, &PayloadOverrides::default());
        assert_eq!(payload["memory_id"], id);
        assert_eq!(payload["project_id"], "default");
        assert_eq!(payload["memory_type"], "semantic");
        assert_eq!(payload["timestamp"], now);
        assert_eq!(payload["text"], "sample");
        assert_eq!(payload["chunk_hash"], chunk_hash);
    }

    #[test]
    fn payload_applies_overrides() {
        let id = generate_memory_id();
        let now = "2025-01-01T00:00:00Z";
        let overrides = PayloadOverrides {
            project_id: Some("proj".into()),
            memory_type: Some("episodic".into()),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            source_uri: Some("file://doc".into()),
            ..Default::default()
        };
        let payload = build_payload(&id, "sample", now, "hash", &overrides);
        assert_eq!(payload["project_id"], "proj");
        assert_eq!(payload["memory_type"], "episodic");
        assert_eq!(payload["source_uri"], "file://doc");
        let tags = payload["tags"].as_array().expect("tags present");
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().any(|tag| tag == "alpha"));
        assert!(tags.iter().any(|tag| tag == "beta"));
    }
}
