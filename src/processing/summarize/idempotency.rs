use crate::qdrant::types::QdrantError;
use crate::qdrant::{self, QdrantService, SearchFilterArgs};
use serde_json::json;

/// Construct the standard idempotency tag used for semantic summaries.
pub(crate) fn idempotency_tag(key: &str) -> String {
    format!("summary:{key}")
}

/// Look up an existing semantic summary that matches the idempotency tag.
pub(crate) async fn find_existing_summary(
    qdrant: &QdrantService,
    collection: &str,
    project_id: Option<&str>,
    tag: &str,
) -> Result<Option<(String, String)>, QdrantError> {
    let filter = qdrant::build_search_filter(&SearchFilterArgs {
        project_id: project_id.map(|value| value.to_string()),
        memory_type: Some("semantic".into()),
        tags: Some(vec![tag.to_string()]),
        time_range: None,
    });

    let existing = qdrant
        .scroll_payloads_with_ids(collection, json!(["text"]), filter)
        .await?;

    Ok(existing.into_iter().next().map(|(id, payload)| {
        let text = payload
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        (id, text)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_tag_applies_prefix() {
        assert_eq!(idempotency_tag("abc"), "summary:abc");
    }
}
