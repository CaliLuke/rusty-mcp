//! Handler and helpers for the `search` tool.

use std::{collections::HashSet, sync::Arc};

use crate::{
    config::get_config,
    mcp::{
        MEMORY_TYPES,
        format::{build_search_response, format_search_hits},
        handlers::parse_arguments_value,
    },
    processing::{ProcessingService, SearchError, SearchRequest, SearchTimeRange},
};
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, JsonObject},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Handle the `search` tool by performing a semantic query against stored memories.
pub(crate) async fn handle_search(
    processing: &Arc<ProcessingService>,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, McpError> {
    let normalized_arguments = normalize_search_arguments(arguments);
    let tags_present = normalized_arguments
        .as_object()
        .map(|map| map.contains_key("tags"))
        .unwrap_or(false);
    let time_range_present = normalized_arguments
        .as_object()
        .map(|map| map.contains_key("time_range"))
        .unwrap_or(false);

    let args: SearchToolRequest = parse_arguments_value(normalized_arguments)?;
    let params = validate_search_request(args, tags_present, time_range_present)?;
    let ValidatedSearchInput {
        query_text,
        project_id,
        memory_type,
        tags,
        time_range,
        limit,
        score_threshold,
        collection,
    } = params;

    let config = get_config();
    let collection_name = collection
        .clone()
        .unwrap_or_else(|| config.qdrant_collection_name.clone());

    let used_filters = build_used_filters(
        &collection_name,
        limit,
        score_threshold,
        project_id.as_ref(),
        memory_type.as_ref(),
        tags.as_ref(),
        time_range.as_ref(),
    );

    let search_request = SearchRequest {
        query_text,
        collection: Some(collection_name.clone()),
        project_id,
        memory_type,
        tags,
        time_range: time_range.clone().map(SearchTimeRange::from),
        limit: Some(limit),
        score_threshold: Some(score_threshold),
    };

    let hits = processing
        .search_memories(search_request)
        .await
        .map_err(map_search_error)?;

    let (results, context) = format_search_hits(hits);
    let payload = build_search_response(
        collection_name,
        limit,
        score_threshold,
        results,
        context,
        used_filters,
    );

    Ok(CallToolResult::structured(payload))
}

/// Raw search request payload accepted from MCP clients.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchToolRequest {
    /// Natural language query text to embed.
    pub(crate) query_text: String,
    /// Optional `project_id` filter.
    #[serde(default)]
    pub(crate) project_id: Option<String>,
    /// Optional memory type filter.
    #[serde(default)]
    pub(crate) memory_type: Option<String>,
    /// Optional tags filter.
    #[serde(default)]
    pub(crate) tags: Option<Vec<String>>,
    /// Optional timestamp range filter.
    #[serde(default)]
    pub(crate) time_range: Option<SearchToolTimeRange>,
    /// Optional limit override.
    #[serde(default)]
    pub(crate) limit: Option<usize>,
    /// Optional score threshold override.
    #[serde(default)]
    pub(crate) score_threshold: Option<f32>,
    /// Optional collection override.
    #[serde(default)]
    pub(crate) collection: Option<String>,
}

/// Timestamp bounds supplied by MCP clients.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchToolTimeRange {
    /// Inclusive start timestamp.
    #[serde(default)]
    pub(crate) start: Option<String>,
    /// Inclusive end timestamp.
    #[serde(default)]
    pub(crate) end: Option<String>,
}

/// Normalized search parameters after validation.
#[derive(Debug)]
pub(crate) struct ValidatedSearchInput {
    /// Query text ready for embedding.
    pub(crate) query_text: String,
    /// Optional project identifier filter.
    pub(crate) project_id: Option<String>,
    /// Optional memory type filter.
    pub(crate) memory_type: Option<String>,
    /// Optional tag filter.
    pub(crate) tags: Option<Vec<String>>,
    /// Optional time-range filter retaining the original representation.
    pub(crate) time_range: Option<SearchToolTimeRange>,
    /// Effective result limit.
    pub(crate) limit: usize,
    /// Effective score threshold.
    pub(crate) score_threshold: f32,
    /// Optional collection override.
    pub(crate) collection: Option<String>,
}

impl From<SearchToolTimeRange> for SearchTimeRange {
    fn from(value: SearchToolTimeRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

/// Normalize search arguments, honoring aliases like `type` and `k`.
pub(crate) fn normalize_search_arguments(arguments: Option<JsonObject>) -> Value {
    let mut map = arguments.unwrap_or_default();

    move_alias(&mut map, "type", "memory_type");
    move_alias(&mut map, "project", "project_id");
    move_alias(&mut map, "k", "limit");

    if let Some(tags_value) = map.remove("tags") {
        match tags_value {
            Value::String(tag) => {
                if !tag.trim().is_empty() {
                    map.insert("tags".into(), Value::Array(vec![Value::String(tag)]));
                }
            }
            Value::Array(items) => {
                map.insert("tags".into(), Value::Array(items));
            }
            other => {
                map.insert("tags".into(), other);
            }
        }
    }

    Value::Object(map)
}

fn move_alias(map: &mut JsonObject, alias: &str, canonical: &str) {
    if let Some(value) = map.remove(alias) {
        if map.contains_key(canonical) {
            tracing::debug!(
                alias = alias,
                canonical = canonical,
                "Alias ignored because canonical key provided"
            );
        } else {
            map.insert(canonical.to_string(), value);
        }
    }
}

fn normalize_tags(
    tags: Option<Vec<String>>,
    provided: bool,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(mut tags_vec) = tags else {
        if provided {
            return Err("`tags` must be an array of non-empty strings");
        }
        return Ok(None);
    };

    let mut normalized = Vec::new();
    let mut seen = HashSet::new();

    for tag in tags_vec.drain(..) {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            return Err("`tags` must be an array of non-empty strings");
        }
        let prepared = trimmed.to_string();
        if seen.insert(prepared.clone()) {
            normalized.push(prepared);
        }
    }

    if normalized.is_empty() {
        return Err("`tags` must be an array of non-empty strings");
    }

    Ok(Some(normalized))
}

fn validate_time_range(
    time_range: Option<SearchToolTimeRange>,
    provided: bool,
) -> Result<Option<SearchToolTimeRange>, McpError> {
    let Some(mut range) = time_range else {
        return Ok(None);
    };

    let parse_timestamp = |label: &str, value: &str| -> Result<OffsetDateTime, McpError> {
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
            McpError::invalid_params(
                format!("`{label}` must be a valid RFC3339 timestamp (got '{value}')"),
                None,
            )
        })
    };

    let mut start_dt = None;
    if let Some(ref mut start) = range.start {
        let trimmed = start.trim();
        if trimmed.is_empty() {
            return Err(McpError::invalid_params(
                "`time_range.start` must be a valid RFC3339 timestamp",
                None,
            ));
        }
        let parsed = parse_timestamp("time_range.start", trimmed)?;
        *start = trimmed.to_string();
        start_dt = Some(parsed);
    }

    let mut end_dt = None;
    if let Some(ref mut end) = range.end {
        let trimmed = end.trim();
        if trimmed.is_empty() {
            return Err(McpError::invalid_params(
                "`time_range.end` must be a valid RFC3339 timestamp",
                None,
            ));
        }
        let parsed = parse_timestamp("time_range.end", trimmed)?;
        *end = trimmed.to_string();
        end_dt = Some(parsed);
    }

    if range.start.is_none() && range.end.is_none() {
        if provided {
            return Err(McpError::invalid_params(
                "`time_range` must include `start`, `end`, or both",
                None,
            ));
        }
        return Ok(None);
    }

    if let (Some(start), Some(end)) = (start_dt, end_dt) {
        if start > end {
            return Err(McpError::invalid_params(
                "`time_range.start` must be earlier than or equal to `time_range.end`",
                None,
            ));
        }
    }

    Ok(Some(range))
}

fn validate_search_request(
    args: SearchToolRequest,
    tags_present: bool,
    time_range_present: bool,
) -> Result<ValidatedSearchInput, McpError> {
    let SearchToolRequest {
        query_text,
        project_id,
        memory_type,
        tags,
        time_range,
        limit,
        score_threshold,
        collection,
    } = args;

    if query_text.trim().is_empty() {
        return Err(McpError::invalid_params(
            "`query_text` must not be empty",
            None,
        ));
    }

    let mut memory_type = memory_type;
    if let Some(ref mut value) = memory_type {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(McpError::invalid_params(
                "`memory_type` must be one of episodic|semantic|procedural",
                None,
            ));
        }
        let normalized = trimmed.to_lowercase();
        if !MEMORY_TYPES.contains(&normalized.as_str()) {
            return Err(McpError::invalid_params(
                "`memory_type` must be one of episodic|semantic|procedural",
                None,
            ));
        }
        *value = normalized;
    }

    let tags = normalize_tags(tags, tags_present)
        .map_err(|message| McpError::invalid_params(message.to_string(), None))?;
    let time_range = validate_time_range(time_range, time_range_present)?;

    let config = get_config();

    if let Some(limit_value) = limit {
        if limit_value < 1 || limit_value > config.search_max_limit {
            return Err(McpError::invalid_params(
                format!("`limit` must be between 1 and {}", config.search_max_limit),
                None,
            ));
        }
    }
    let limit_value = limit.unwrap_or(config.search_default_limit);

    if let Some(threshold) = score_threshold {
        if !(0.0..=1.0).contains(&threshold) {
            return Err(McpError::invalid_params(
                "`score_threshold` must be between 0.0 and 1.0",
                None,
            ));
        }
    }
    let threshold_value = score_threshold.unwrap_or(config.search_default_score_threshold);

    Ok(ValidatedSearchInput {
        query_text,
        project_id,
        memory_type,
        tags,
        time_range,
        limit: limit_value,
        score_threshold: threshold_value,
        collection,
    })
}

fn build_used_filters(
    collection: &str,
    limit: usize,
    score_threshold: f32,
    project_id: Option<&String>,
    memory_type: Option<&String>,
    tags: Option<&Vec<String>>,
    time_range: Option<&SearchToolTimeRange>,
) -> Map<String, Value> {
    let mut filters = Map::new();

    if let Some(project) = project_id {
        filters.insert("project_id".into(), Value::String(project.clone()));
    }
    if let Some(memory) = memory_type {
        filters.insert("memory_type".into(), Value::String(memory.clone()));
    }
    if let Some(tags_value) = tags.filter(|values| !values.is_empty()) {
        filters.insert("tags".into(), json!(tags_value));
    }
    if let Some(range) = time_range {
        let mut range_map = Map::new();
        if let Some(start) = range.start.as_ref() {
            range_map.insert("start".into(), Value::String(start.clone()));
        }
        if let Some(end) = range.end.as_ref() {
            range_map.insert("end".into(), Value::String(end.clone()));
        }
        if !range_map.is_empty() {
            filters.insert("time_range".into(), Value::Object(range_map));
        }
    }

    filters.insert("collection".into(), Value::String(collection.to_string()));
    filters.insert("limit".into(), Value::from(limit as u64));
    filters.insert("score_threshold".into(), json!(score_threshold));

    filters
}

fn map_search_error(error: SearchError) -> McpError {
    match error {
        SearchError::Embedding(source) => {
            McpError::internal_error(format!("Embedding provider error: {source}"), None)
        }
        SearchError::Qdrant(source) => {
            McpError::internal_error(format!("Qdrant request failed: {source}"), None)
        }
        SearchError::DimensionMismatch { expected, actual } => McpError::internal_error(
            format!(
                "Embedding dimension mismatch: expected {expected}, got {actual}. Align EMBEDDING_MODEL and EMBEDDING_DIMENSION."
            ),
            None,
        ),
        SearchError::EmptyEmbedding => McpError::internal_error(
            "Embedding provider returned no vectors for the query.",
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONFIG, Config, EmbeddingProvider};
    use crate::processing::SearchHit;
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

    fn base_search_request() -> SearchToolRequest {
        SearchToolRequest {
            query_text: "demo".into(),
            project_id: None,
            memory_type: None,
            tags: None,
            time_range: None,
            limit: None,
            score_threshold: None,
            collection: None,
        }
    }

    #[test]
    fn normalize_search_arguments_supports_aliases_and_tags() {
        ensure_test_config();
        let mut raw = JsonObject::new();
        raw.insert("query_text".into(), Value::String("demo".into()));
        raw.insert("type".into(), Value::String("semantic".into()));
        raw.insert("memory_type".into(), Value::String("episodic".into()));
        raw.insert("project".into(), Value::String("alpha".into()));
        raw.insert("k".into(), Value::from(3));
        raw.insert("tags".into(), Value::String(" docs ".into()));

        let normalized = normalize_search_arguments(Some(raw));
        let mut request: SearchToolRequest =
            parse_arguments_value(normalized).expect("normalized arguments parse");
        request.tags =
            normalize_tags(request.tags, true).expect("tags normalization should succeed");

        assert_eq!(request.memory_type.as_deref(), Some("episodic"));
        assert_eq!(request.project_id.as_deref(), Some("alpha"));
        assert_eq!(request.limit, Some(3));
        assert_eq!(request.tags, Some(vec!["docs".into()]));
    }

    #[test]
    fn validate_search_request_rejects_empty_query() {
        ensure_test_config();
        let request = SearchToolRequest {
            query_text: "   ".into(),
            ..base_search_request()
        };
        let error = validate_search_request(request, false, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_search_request_rejects_invalid_memory_type() {
        ensure_test_config();
        let request = SearchToolRequest {
            query_text: "demo".into(),
            memory_type: Some("invalid".into()),
            ..base_search_request()
        };
        let error = validate_search_request(request, false, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_search_request_rejects_limit_out_of_bounds() {
        ensure_test_config();
        let mut request = base_search_request();
        request.query_text = "demo".into();
        request.limit = Some(0);
        let error = validate_search_request(request, false, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_search_request_rejects_score_threshold_out_of_range() {
        ensure_test_config();
        let mut request = base_search_request();
        request.query_text = "demo".into();
        request.score_threshold = Some(1.5);
        let error = validate_search_request(request, false, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_search_request_rejects_empty_tags() {
        ensure_test_config();
        let mut request = base_search_request();
        request.query_text = "demo".into();
        request.tags = Some(vec![" ".into()]);
        let error = validate_search_request(request, true, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn build_used_filters_includes_defaults_and_filters() {
        ensure_test_config();
        let project = "alpha".to_string();
        let memory = "semantic".to_string();
        let tags = vec!["docs".to_string(), "api".to_string()];
        let time_range = SearchToolTimeRange {
            start: Some("2024-01-01T00:00:00Z".into()),
            end: None,
        };

        let filters = build_used_filters(
            "rusty",
            7,
            0.4,
            Some(&project),
            Some(&memory),
            Some(&tags),
            Some(&time_range),
        );

        assert_eq!(
            filters.get("project_id").and_then(Value::as_str),
            Some("alpha")
        );
        assert_eq!(
            filters.get("memory_type").and_then(Value::as_str),
            Some("semantic")
        );
        assert_eq!(
            filters.get("collection").and_then(Value::as_str),
            Some("rusty")
        );
        assert_eq!(filters.get("limit").and_then(Value::as_u64), Some(7));
        let score_value = filters
            .get("score_threshold")
            .and_then(Value::as_f64)
            .expect("score");
        assert!((score_value - 0.4).abs() < 1e-6);
        let tag_values: Vec<&str> = filters
            .get("tags")
            .and_then(Value::as_array)
            .expect("tags array")
            .iter()
            .map(|value| value.as_str().expect("tag string"))
            .collect();
        assert_eq!(tag_values, ["docs", "api"]);
        let time_value = filters
            .get("time_range")
            .and_then(Value::as_object)
            .expect("time range object");
        assert_eq!(time_value["start"], "2024-01-01T00:00:00Z");
        assert!(!time_value.contains_key("end"));
    }

    #[test]
    fn format_search_hits_builds_context_with_citations() {
        let hit = SearchHit {
            id: "chunk-1".into(),
            score: 0.42,
            text: Some("Example text".into()),
            project_id: None,
            memory_type: None,
            tags: None,
            timestamp: None,
            source_uri: None,
        };
        let (results, context) = format_search_hits(vec![hit]);
        assert_eq!(results.len(), 1);
        assert_eq!(context.as_deref(), Some("Example text [chunk-1]"));
    }

    #[test]
    fn map_search_error_wraps_embedding_errors() {
        let error = SearchError::Embedding(
            crate::embedding::EmbeddingClientError::GenerationFailed("fail".into()),
        );
        let mapped = map_search_error(error);
        assert_eq!(mapped.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert!(mapped.message.contains("Embedding provider error"));
    }
}
