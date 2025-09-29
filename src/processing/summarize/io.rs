use crate::qdrant::types::{PayloadOverrides, QdrantError};
use crate::qdrant::{self, PointInsert, QdrantService, SearchFilterArgs};
use serde_json::{Map, Value, json};

use super::{EpisodicMemory, sort_memories};

/// Fetch episodic memories from Qdrant, sort chronologically, and apply the requested limit.
pub(crate) async fn fetch_episodic_items(
    qdrant: &QdrantService,
    collection: &str,
    fields: Value,
    filter: Option<Value>,
    limit: usize,
) -> Result<Vec<EpisodicMemory>, QdrantError> {
    let mut items = qdrant
        .scroll_payloads_with_ids(collection, fields, filter)
        .await?
        .into_iter()
        .filter_map(|(id, payload)| map_payload_into_memory(id, payload))
        .collect::<Vec<_>>();

    sort_memories(&mut items);
    if items.len() > limit {
        items.truncate(limit);
    }

    Ok(items)
}

fn map_payload_into_memory(
    memory_id: String,
    payload: Map<String, Value>,
) -> Option<EpisodicMemory> {
    let text = payload
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return None;
    }

    let timestamp = payload
        .get("timestamp")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    Some(EpisodicMemory::new(memory_id, text, timestamp))
}

/// Persist a semantic summary and resolve the resulting memory identifier.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_semantic_summary(
    qdrant: &QdrantService,
    collection: &str,
    summary_text: &str,
    vector: Vec<f32>,
    chunk_hash: String,
    overrides: &PayloadOverrides,
    project_id: Option<&str>,
    idempotency_tag: &str,
) -> Result<String, QdrantError> {
    qdrant
        .index_points(
            collection,
            vec![PointInsert {
                text: summary_text.to_string(),
                chunk_hash,
                vector,
            }],
            overrides,
        )
        .await?;

    let filter = qdrant::build_search_filter(&SearchFilterArgs {
        project_id: project_id.map(|value| value.to_string()),
        memory_type: Some("semantic".into()),
        tags: Some(vec![idempotency_tag.to_string()]),
        time_range: None,
    });

    let resolve = qdrant
        .scroll_payloads_with_ids(collection, json!(["text"]), filter)
        .await?;

    Ok(resolve
        .into_iter()
        .map(|(id, _)| id)
        .next()
        .unwrap_or_default())
}
