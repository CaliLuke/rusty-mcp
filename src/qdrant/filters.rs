//! Filter helpers for Qdrant search queries and payload accumulation.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::types::SearchFilterArgs;

/// Compose the standard Qdrant filter payload from optional search arguments.
pub fn build_search_filter(args: &SearchFilterArgs) -> Option<Value> {
    let mut must: Vec<Value> = Vec::new();

    if let Some(project_id) = args.project_id.as_ref().and_then(|value| non_empty(value)) {
        must.push(json!({
            "key": "project_id",
            "match": { "value": project_id }
        }));
    }

    if let Some(memory_type) = args.memory_type.as_ref().and_then(|value| non_empty(value)) {
        must.push(json!({
            "key": "memory_type",
            "match": { "value": memory_type }
        }));
    }

    if let Some(tags) = args.tags.as_ref() {
        let cleaned: Vec<String> = tags
            .iter()
            .filter_map(|tag| non_empty(tag).map(|value| value.to_string()))
            .collect();
        if !cleaned.is_empty() {
            must.push(json!({
                "key": "tags",
                "match": { "any": cleaned }
            }));
        }
    }

    if let Some(range) = args.time_range.as_ref() {
        let mut boundaries = Map::new();
        if let Some(start) = range.start.as_ref().and_then(|value| non_empty(value)) {
            boundaries.insert("gte".into(), Value::String(start.to_string()));
        }
        if let Some(end) = range.end.as_ref().and_then(|value| non_empty(value)) {
            boundaries.insert("lte".into(), Value::String(end.to_string()));
        }
        if !boundaries.is_empty() {
            must.push(json!({
                "key": "timestamp",
                "range": Value::Object(boundaries)
            }));
        }
    }

    if must.is_empty() {
        None
    } else {
        Some(json!({ "must": must }))
    }
}

fn non_empty(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Accumulate project identifiers from Qdrant payloads.
pub fn accumulate_project_id(payload: &Map<String, Value>, projects: &mut BTreeSet<String>) {
    if let Some(Value::String(project)) = payload.get("project_id") {
        let trimmed = project.trim();
        if !trimmed.is_empty() {
            projects.insert(trimmed.to_string());
        }
    }
}

/// Accumulate tag values from Qdrant payloads.
pub fn accumulate_tags(payload: &Map<String, Value>, tags: &mut BTreeSet<String>) {
    match payload.get("tags") {
        Some(Value::Array(values)) => {
            for value in values {
                if let Value::String(tag) = value {
                    let trimmed = tag.trim();
                    if !trimmed.is_empty() {
                        tags.insert(trimmed.to_string());
                    }
                }
            }
        }
        Some(Value::String(tag)) => {
            let trimmed = tag.trim();
            if !trimmed.is_empty() {
                tags.insert(trimmed.to_string());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::SearchTimeRange;
    use super::*;

    #[test]
    fn build_search_filter_handles_project_id() {
        let filter = build_search_filter(&SearchFilterArgs {
            project_id: Some("repo-a".into()),
            ..Default::default()
        })
        .expect("filter");

        assert_eq!(
            filter,
            json!({
                "must": [
                    {
                        "key": "project_id",
                        "match": { "value": "repo-a" }
                    }
                ]
            })
        );
    }

    #[test]
    fn build_search_filter_handles_tags() {
        let filter = build_search_filter(&SearchFilterArgs {
            tags: Some(vec!["alpha".into(), "beta".into()]),
            ..Default::default()
        })
        .expect("filter");

        assert_eq!(
            filter,
            json!({
                "must": [
                    {
                        "key": "tags",
                        "match": { "any": ["alpha", "beta"] }
                    }
                ]
            })
        );
    }

    #[test]
    fn build_search_filter_handles_time_range() {
        let filter = build_search_filter(&SearchFilterArgs {
            time_range: Some(SearchTimeRange {
                start: Some("2025-01-01T00:00:00Z".into()),
                end: Some("2025-12-31T23:59:59Z".into()),
            }),
            ..Default::default()
        })
        .expect("filter");

        assert_eq!(
            filter,
            json!({
                "must": [
                    {
                        "key": "timestamp",
                        "range": {
                            "gte": "2025-01-01T00:00:00Z",
                            "lte": "2025-12-31T23:59:59Z"
                        }
                    }
                ]
            })
        );
    }

    #[test]
    fn build_search_filter_returns_none_when_empty() {
        assert!(build_search_filter(&SearchFilterArgs::default()).is_none());
    }

    #[test]
    fn accumulate_project_ignores_empty() {
        let mut map = Map::new();
        map.insert("project_id".into(), Value::String("   ".into()));
        let mut projects = BTreeSet::new();
        accumulate_project_id(&map, &mut projects);
        assert!(projects.is_empty());

        map.insert("project_id".into(), Value::String("repo-a".into()));
        accumulate_project_id(&map, &mut projects);
        assert_eq!(
            projects.iter().collect::<Vec<_>>(),
            vec![&"repo-a".to_string()]
        );
    }

    #[test]
    fn accumulate_tags_handles_arrays_and_strings() {
        let mut map = Map::new();
        map.insert(
            "tags".into(),
            Value::Array(vec![
                Value::String("alpha".into()),
                Value::String("".into()),
            ]),
        );
        let mut tags = BTreeSet::new();
        accumulate_tags(&map, &mut tags);
        assert_eq!(tags.iter().collect::<Vec<_>>(), vec![&"alpha".to_string()]);

        map.insert("tags".into(), Value::String(" beta  ".into()));
        accumulate_tags(&map, &mut tags);
        assert_eq!(
            tags.iter().collect::<Vec<_>>(),
            vec![&"alpha".to_string(), &"beta".to_string()]
        );
    }
}
