//! Helpers for normalizing metadata values.

use crate::qdrant::PayloadOverrides;
use serde_json::{Map, Value};
use std::collections::HashSet;

use super::types::IngestMetadata;

/// Sanitize arbitrary string input by trimming whitespace and dropping empties.
pub(crate) fn sanitize_string(value: Option<String>) -> Option<String> {
    value.and_then(|input| {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Normalize `project_id` values, falling back to the configured default when absent.
pub fn sanitize_project_id(value: Option<String>) -> Option<String> {
    sanitize_string(value).or_else(|| Some("default".into()))
}

/// Normalize `memory_type` values to the known variants.
pub fn sanitize_memory_type(value: Option<String>) -> Option<String> {
    sanitize_string(value).and_then(|candidate| {
        let normalized = candidate.to_lowercase();
        match normalized.as_str() {
            "episodic" | "semantic" | "procedural" => Some(normalized),
            _ => None,
        }
    })
}

/// Normalize and dedupe tag values, dropping empties.
pub fn sanitize_tags(values: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut unique = HashSet::new();
    let mut sanitized = Vec::new();
    let Some(items) = values else {
        return None;
    };

    for tag in items {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if unique.insert(lower.clone()) {
            sanitized.push(lower);
        }
    }

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

/// Extract tag values from a Qdrant payload map.
pub fn extract_tags(payload: &Map<String, Value>) -> Option<Vec<String>> {
    match payload.get("tags") {
        Some(Value::Array(values)) if !values.is_empty() => {
            let tags: Vec<String> = values
                .iter()
                .filter_map(|value| value.as_str().map(|s| s.trim().to_string()))
                .filter(|tag| !tag.is_empty())
                .collect();
            if tags.is_empty() { None } else { Some(tags) }
        }
        Some(Value::String(tag)) => {
            let trimmed = tag.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        _ => None,
    }
}

/// Convert ingest metadata into Qdrant payload overrides.
pub(crate) fn to_payload_overrides(metadata: IngestMetadata) -> PayloadOverrides {
    let IngestMetadata {
        project_id,
        memory_type,
        tags,
        source_uri,
    } = metadata;

    PayloadOverrides {
        project_id: sanitize_project_id(project_id),
        memory_type: sanitize_memory_type(memory_type),
        tags: sanitize_tags(tags),
        source_uri: sanitize_string(source_uri),
        source_memory_ids: None,
        summary_key: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_project_id_trims_and_defaults() {
        assert_eq!(
            sanitize_project_id(Some("  proj  ".into())),
            Some("proj".into())
        );
        assert_eq!(sanitize_project_id(None), Some("default".into()));
    }

    #[test]
    fn sanitize_memory_type_filters_invalid() {
        assert_eq!(
            sanitize_memory_type(Some("Episodic".into())),
            Some("episodic".into())
        );
        assert!(sanitize_memory_type(Some("unknown".into())).is_none());
    }

    #[test]
    fn sanitize_tags_uniquifies_and_trims() {
        let tags = sanitize_tags(Some(vec![
            "alpha".into(),
            " beta".into(),
            "alpha".into(),
            "".into(),
        ]));
        assert_eq!(tags.as_ref().map(|t| t.len()), Some(2));
        let values = tags.unwrap();
        assert!(values.contains(&"alpha".into()));
        assert!(values.contains(&"beta".into()));
    }

    #[test]
    fn extract_tags_handles_string_value() {
        let mut payload = Map::new();
        payload.insert("tags".into(), Value::String(" single ".into()));
        let tags = extract_tags(&payload).expect("tags");
        assert_eq!(tags, vec!["single".to_string()]);

        payload.insert("tags".into(), serde_json::json!(["alpha", "", "beta"]));
        let tags = extract_tags(&payload).expect("array tags");
        assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);
    }
}
