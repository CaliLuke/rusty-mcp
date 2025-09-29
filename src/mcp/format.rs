//! Formatting helpers shared across MCP handlers and resources.

use crate::{
    config::EmbeddingProvider,
    processing::{QdrantHealthSnapshot, SearchHit},
};
use rmcp::model::ResourceContents;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

pub(crate) const APPLICATION_JSON: &str = "application/json";

/// Format the static memory types manifest returned via MCP resources.
pub(crate) fn memory_types_payload() -> String {
    serde_json::to_string_pretty(&json!({
        "memory_types": ["episodic", "semantic", "procedural"],
        "default": "semantic"
    }))
    .unwrap_or_else(|_| {
        "{\"memory_types\":[\"episodic\",\"semantic\",\"procedural\"],\"default\":\"semantic\"}"
            .into()
    })
}

/// Build the health payload summarizing embedding and Qdrant status.
pub(crate) fn health_payload(
    provider: EmbeddingProvider,
    model: &str,
    dimension: usize,
    qdrant_url: &str,
    default_collection: &str,
    snapshot: &QdrantHealthSnapshot,
) -> String {
    let mut qdrant = Map::new();
    qdrant.insert("url".into(), Value::String(qdrant_url.to_string()));
    qdrant.insert("reachable".into(), Value::Bool(snapshot.reachable));
    qdrant.insert(
        "defaultCollection".into(),
        Value::String(default_collection.to_string()),
    );
    qdrant.insert(
        "defaultCollectionPresent".into(),
        Value::Bool(snapshot.default_collection_present),
    );
    if let Some(error) = snapshot.error.as_ref() {
        qdrant.insert("error".into(), Value::String(error.clone()));
    }

    let payload = json!({
        "embedding": {
            "provider": embedding_provider_label(provider),
            "model": model,
            "dimension": dimension,
        },
        "qdrant": Value::Object(qdrant),
    });

    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
}

fn embedding_provider_label(provider: EmbeddingProvider) -> &'static str {
    match provider {
        EmbeddingProvider::Ollama => "ollama",
        EmbeddingProvider::OpenAI => "openai",
    }
}

/// Serialize a value to JSON, falling back to compact formatting on error.
pub(crate) fn serialize_json<T: Serialize>(value: &T, context_uri: &str) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| {
        tracing::warn!(uri = context_uri, %error, "Failed to serialize JSON prettily");
        serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
    })
}

/// Build JSON resource contents for MCP resource responses.
pub(crate) fn json_resource_contents(uri: &str, text: String) -> ResourceContents {
    ResourceContents::TextResourceContents {
        uri: uri.to_string(),
        mime_type: Some(APPLICATION_JSON.into()),
        text,
        meta: None,
    }
}

/// Projects snapshot returned by the `projects` resource.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ProjectsSnapshot {
    /// Ordered list of project identifiers.
    pub(crate) projects: Vec<String>,
}

/// Project tags snapshot returned by the templated resource.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ProjectTagsSnapshot {
    /// Project identifier used to scope the tags.
    pub(crate) project_id: String,
    /// Tags observed for the project.
    pub(crate) tags: Vec<String>,
}

/// Top-level settings snapshot describing search defaults.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SettingsSnapshot {
    /// Search-specific defaults.
    pub(crate) search: SearchSettingsSnapshot,
}

/// Structure describing search defaults for clients.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SearchSettingsSnapshot {
    /// Default search limit when callers omit it.
    pub(crate) default_limit: usize,
    /// Maximum limit supported by the server configuration.
    pub(crate) max_limit: usize,
    /// Default score threshold when callers omit it.
    pub(crate) default_score_threshold: f32,
}

/// Format search hits into MCP response payloads and a prompt-ready context string.
pub(crate) fn format_search_hits(hits: Vec<SearchHit>) -> (Vec<Value>, Option<String>) {
    let mut results = Vec::with_capacity(hits.len());
    let mut context_segments = Vec::new();

    for hit in hits {
        let mut item = Map::new();
        let id = hit.id;
        item.insert("id".into(), Value::String(id.clone()));
        item.insert("score".into(), json!(hit.score));

        if let Some(text) = hit.text {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                context_segments.push(format!("{trimmed} [{id}]"));
            }
            item.insert("text".into(), Value::String(text));
        }
        if let Some(project_id) = hit.project_id {
            item.insert("project_id".into(), Value::String(project_id));
        }
        if let Some(memory_type) = hit.memory_type {
            item.insert("memory_type".into(), Value::String(memory_type));
        }
        if let Some(tags) = hit.tags {
            item.insert("tags".into(), json!(tags));
        }
        if let Some(timestamp) = hit.timestamp {
            item.insert("timestamp".into(), Value::String(timestamp));
        }
        if let Some(source_uri) = hit.source_uri {
            item.insert("source_uri".into(), Value::String(source_uri));
        }

        results.push(Value::Object(item));
    }

    let context = if context_segments.is_empty() {
        None
    } else {
        Some(context_segments.join("\n"))
    };

    (results, context)
}

/// Assemble the full structured search response.
pub(crate) fn build_search_response(
    collection_name: String,
    limit: usize,
    score_threshold: f32,
    results: Vec<Value>,
    context: Option<String>,
    used_filters: Map<String, Value>,
) -> Value {
    let mut payload = Map::new();
    payload.insert("results".into(), Value::Array(results));
    payload.insert("collection".into(), Value::String(collection_name));
    payload.insert("limit".into(), Value::from(limit as u64));
    payload.insert("score_threshold".into(), json!(score_threshold));
    payload.insert("scoreThreshold".into(), json!(score_threshold));
    payload.insert("used_filters".into(), Value::Object(used_filters));
    if let Some(context_value) = context {
        payload.insert("context".into(), Value::String(context_value));
    }

    Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONFIG, Config, EmbeddingProvider};
    use crate::processing::QdrantHealthSnapshot;
    use serde_json::Value;
    use std::sync::Once;

    fn ensure_test_config() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = CONFIG.set(Config {
                qdrant_url: "http://127.0.0.1:6333".into(),
                qdrant_collection_name: "rusty-mem".into(),
                qdrant_api_key: None,
                embedding_provider: EmbeddingProvider::Ollama,
                text_splitter_chunk_size: None,
                text_splitter_chunk_overlap: None,
                text_splitter_use_safe_defaults: false,
                embedding_model: "test-model".into(),
                embedding_dimension: 768,
                ollama_url: None,
                server_port: None,
                search_default_limit: 5,
                search_max_limit: 50,
                search_default_score_threshold: 0.25,
            });
        });
    }

    #[test]
    fn memory_types_payload_is_valid_json() {
        let body = memory_types_payload();
        let value: Value =
            serde_json::from_str(&body).expect("memory-types payload must be valid JSON");
        assert_eq!(value["default"], "semantic");
        let memory_types = value["memory_types"]
            .as_array()
            .expect("memory_types array");
        assert_eq!(memory_types.len(), 3);
    }

    #[test]
    fn health_payload_captures_qdrant_status() {
        ensure_test_config();
        let snapshot = QdrantHealthSnapshot {
            reachable: false,
            default_collection_present: false,
            error: Some("connection refused".into()),
        };

        let body = health_payload(
            EmbeddingProvider::Ollama,
            "nomic-embed-text",
            768,
            "http://127.0.0.1:6333",
            "rusty-mem",
            &snapshot,
        );

        let value: Value = serde_json::from_str(&body).expect("health payload must be valid JSON");
        assert_eq!(value["embedding"]["provider"], "ollama");
        assert_eq!(value["embedding"]["dimension"], 768);
        assert_eq!(value["qdrant"]["reachable"], false);
        assert_eq!(value["qdrant"]["error"], "connection refused");
    }
}
