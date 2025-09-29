//! Handler for the `summarize` MCP tool.

use std::{collections::HashSet, sync::Arc};

use crate::{
    config::get_config,
    mcp::{MEMORY_TYPES, format::build_summarize_response, handlers::parse_arguments_value},
    processing::{ProcessingService, SummarizeError, SummarizeRequest, SummarizeStrategy},
};
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, JsonObject},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Handle the `summarize` tool invocation.
pub(crate) async fn handle_summarize(
    processing: &Arc<ProcessingService>,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, McpError> {
    let normalized_arguments = normalize_summarize_arguments(arguments);
    let tags_present = normalized_arguments
        .as_object()
        .map(|map| map.contains_key("tags"))
        .unwrap_or(false);

    let args: SummarizeToolRequest = parse_arguments_value(normalized_arguments)?;
    let params = validate_summarize_request(args, tags_present)?;
    let ValidatedSummarizeInput {
        project_id,
        memory_type,
        tags,
        time_range,
        limit,
        strategy,
        provider,
        model,
        max_words,
        collection,
    } = params;

    let project_id_for_filters = project_id.clone();
    let memory_type_for_filters = memory_type.clone();
    let provider_for_filters = provider.clone();
    let model_for_filters = model.clone();
    let tags_for_filters = tags.clone();
    let time_range_for_filters = time_range.clone();
    let collection_name = collection
        .clone()
        .unwrap_or_else(|| get_config().qdrant_collection_name.clone());

    let request = SummarizeRequest {
        project_id,
        memory_type,
        tags,
        time_range: time_range.into(),
        limit: Some(limit),
        strategy: Some(strategy.clone()),
        provider,
        model,
        max_words: Some(max_words),
        collection: collection.clone(),
    };

    let outcome = processing
        .summarize_memories(request)
        .await
        .map_err(map_summarize_error)?;

    let used_filters = build_used_filters(SummarizeFilterContext {
        collection: collection_name,
        project_id: project_id_for_filters,
        memory_type: memory_type_for_filters,
        tags: tags_for_filters,
        time_range: time_range_for_filters,
        limit,
        max_words,
        strategy,
        provider: provider_for_filters,
        model: model_for_filters,
    });

    let payload = build_summarize_response(outcome, used_filters);
    Ok(CallToolResult::structured(payload))
}

/// Raw request payload accepted from MCP clients.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummarizeToolRequest {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    time_range: SummarizeToolTimeRange,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    strategy: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_words: Option<usize>,
    #[serde(default)]
    _score_threshold: Option<f32>,
    #[serde(default)]
    collection: Option<String>,
}

/// Timestamp bounds supplied by the tool request.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct SummarizeToolTimeRange {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

/// Validated summarize input
#[derive(Debug)]
struct ValidatedSummarizeInput {
    project_id: Option<String>,
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
    time_range: SummarizeToolTimeRange,
    limit: usize,
    strategy: SummarizeStrategy,
    provider: Option<String>,
    model: Option<String>,
    max_words: usize,
    collection: Option<String>,
}

struct SummarizeFilterContext {
    collection: String,
    project_id: Option<String>,
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
    time_range: SummarizeToolTimeRange,
    limit: usize,
    max_words: usize,
    strategy: SummarizeStrategy,
    provider: Option<String>,
    model: Option<String>,
}

fn normalize_summarize_arguments(arguments: Option<JsonObject>) -> Value {
    let mut map = arguments.unwrap_or_default();

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

fn validate_summarize_request(
    args: SummarizeToolRequest,
    tags_present: bool,
) -> Result<ValidatedSummarizeInput, McpError> {
    let SummarizeToolRequest {
        mut project_id,
        mut memory_type,
        tags,
        time_range,
        limit,
        strategy,
        provider,
        model,
        max_words,
        _score_threshold,
        collection,
    } = args;

    if let Some(ref mut project) = project_id {
        let trimmed = project.trim();
        if trimmed.is_empty() {
            return Err(McpError::invalid_params(
                "`project_id` must not be empty",
                None,
            ));
        }
        *project = trimmed.to_string();
    }

    if let Some(ref mut memory) = memory_type {
        let trimmed = memory.trim();
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
        *memory = normalized;
    }

    let normalized_tags = normalize_tags(tags, tags_present)?;
    let validated_range = validate_time_range(time_range)?;

    let config = get_config();
    let limit_value = limit.unwrap_or(50);
    if limit_value == 0 || limit_value > config.search_max_limit {
        return Err(McpError::invalid_params(
            format!("`limit` must be between 1 and {}", config.search_max_limit),
            None,
        ));
    }

    let max_words = max_words.unwrap_or(config.summarization_max_words);
    if max_words == 0 {
        return Err(McpError::invalid_params(
            "`max_words` must be greater than zero",
            None,
        ));
    }

    let strategy = strategy
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_else(|| "auto".into());
    let strategy = match strategy.as_str() {
        "auto" => SummarizeStrategy::Auto,
        "abstractive" => SummarizeStrategy::Abstractive,
        "extractive" => SummarizeStrategy::Extractive,
        other => {
            return Err(McpError::invalid_params(
                format!("`strategy` must be auto|abstractive|extractive (got '{other}')"),
                None,
            ));
        }
    };

    if let Some(ref provider_value) = provider {
        let normalized = provider_value.trim().to_lowercase();
        if !matches!(normalized.as_str(), "ollama" | "none") {
            return Err(McpError::invalid_params(
                "`provider` must be one of ollama|none",
                None,
            ));
        }
    }

    Ok(ValidatedSummarizeInput {
        project_id,
        memory_type,
        tags: normalized_tags,
        time_range: validated_range,
        limit: limit_value,
        strategy,
        provider,
        model,
        max_words,
        collection,
    })
}

fn normalize_tags(
    tags: Option<Vec<String>>,
    provided: bool,
) -> Result<Option<Vec<String>>, McpError> {
    let Some(values) = tags else {
        if provided {
            return Err(McpError::invalid_params(
                "`tags` must be an array of non-empty strings",
                None,
            ));
        }
        return Ok(None);
    };

    let mut normalized = Vec::new();
    let mut seen = HashSet::new();

    for tag in values {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            return Err(McpError::invalid_params(
                "`tags` must be an array of non-empty strings",
                None,
            ));
        }
        let prepared = trimmed.to_string();
        if seen.insert(prepared.clone()) {
            normalized.push(prepared);
        }
    }

    Ok(Some(normalized))
}

fn validate_time_range(range: SummarizeToolTimeRange) -> Result<SummarizeToolTimeRange, McpError> {
    let SummarizeToolTimeRange { mut start, mut end } = range;

    let parse_timestamp = |label: &str, value: &str| -> Result<String, McpError> {
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
            McpError::invalid_params(
                format!("`{label}` must be a valid RFC3339 timestamp (got '{value}')"),
                None,
            )
        })?;
        Ok(value.trim().to_string())
    };

    match (start.as_mut(), end.as_mut()) {
        (Some(start_value), Some(end_value)) => {
            *start_value = parse_timestamp("time_range.start", start_value.trim())?;
            *end_value = parse_timestamp("time_range.end", end_value.trim())?;
        }
        _ => {
            return Err(McpError::invalid_params(
                "`time_range` must include both `start` and `end`",
                None,
            ));
        }
    }

    if let (Some(start), Some(end)) = (&start, &end) {
        let start_dt = OffsetDateTime::parse(start, &Rfc3339).unwrap();
        let end_dt = OffsetDateTime::parse(end, &Rfc3339).unwrap();
        if start_dt > end_dt {
            return Err(McpError::invalid_params(
                "`time_range.start` must be earlier than or equal to `time_range.end`",
                None,
            ));
        }
    }

    Ok(SummarizeToolTimeRange { start, end })
}

fn build_used_filters(context: SummarizeFilterContext) -> Map<String, Value> {
    let SummarizeFilterContext {
        collection,
        project_id,
        memory_type,
        tags,
        time_range,
        limit,
        max_words,
        strategy,
        provider,
        model,
    } = context;

    let mut filters = Map::new();
    filters.insert("collection".into(), Value::String(collection));
    filters.insert("limit".into(), Value::from(limit as u64));
    filters.insert("max_words".into(), Value::from(max_words as u64));
    filters.insert(
        "strategy".into(),
        Value::String(strategy_to_string(strategy).into()),
    );

    if let Some(project) = project_id {
        filters.insert("project_id".into(), Value::String(project));
    }
    if let Some(memory) = memory_type {
        filters.insert("memory_type".into(), Value::String(memory));
    }

    if let Some(tags_value) = tags.filter(|values| !values.is_empty()) {
        filters.insert("tags".into(), json!(tags_value));
    }

    let mut range_map = Map::new();
    if let Some(start) = time_range.start {
        range_map.insert("start".into(), Value::String(start));
    }
    if let Some(end) = time_range.end {
        range_map.insert("end".into(), Value::String(end));
    }
    if !range_map.is_empty() {
        filters.insert("time_range".into(), Value::Object(range_map));
    }

    if let Some(provider_value) = provider {
        filters.insert("provider".into(), Value::String(provider_value));
    }
    if let Some(model_value) = model {
        filters.insert("model".into(), Value::String(model_value));
    }

    filters
}

fn map_summarize_error(error: SummarizeError) -> McpError {
    match error {
        SummarizeError::GenerationFailed(message) => McpError::internal_error(message, None),
        SummarizeError::EmptyResult => {
            McpError::invalid_params("No episodic memories found for the requested scope", None)
        }
        SummarizeError::InvalidTimeRange => {
            McpError::invalid_params("`time_range` must include both `start` and `end`", None)
        }
        SummarizeError::Embedding(source) => {
            McpError::internal_error(format!("Embedding provider error: {source}"), None)
        }
        SummarizeError::Qdrant(source) => {
            McpError::internal_error(format!("Qdrant request failed: {source}"), None)
        }
    }
}

fn strategy_to_string(strategy: SummarizeStrategy) -> &'static str {
    match strategy {
        SummarizeStrategy::Auto => "auto",
        SummarizeStrategy::Abstractive => "abstractive",
        SummarizeStrategy::Extractive => "extractive",
    }
}

impl From<SummarizeToolTimeRange> for crate::processing::SearchTimeRange {
    fn from(value: SummarizeToolTimeRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONFIG, Config, EmbeddingProvider, SummarizationProvider};
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
                summarization_provider: SummarizationProvider::Ollama,
                summarization_model: Some("llama".into()),
                summarization_max_words: 200,
            });
        });
    }

    #[test]
    fn normalize_arguments_converts_scalar_tags() {
        let mut args = JsonObject::new();
        args.insert("tags".into(), Value::String("daily".into()));
        args.insert(
            "time_range".into(),
            json!({ "start": "2025-01-01T00:00:00Z", "end": "2025-01-02T00:00:00Z" }),
        );
        let value = normalize_summarize_arguments(Some(args));
        let request: SummarizeToolRequest = parse_arguments_value(value).expect("deserialize");
        assert_eq!(request.tags, Some(vec!["daily".into()]));
    }

    #[test]
    fn validate_summarize_request_honors_defaults() {
        ensure_test_config();
        let request = SummarizeToolRequest {
            project_id: Some(" default ".into()),
            memory_type: Some("Episodic".into()),
            tags: Some(vec!["daily".into()]),
            time_range: SummarizeToolTimeRange {
                start: Some("2025-01-01T00:00:00Z".into()),
                end: Some("2025-01-02T00:00:00Z".into()),
            },
            limit: Some(20),
            strategy: Some("AUTO".into()),
            provider: Some("ollama".into()),
            model: Some("llama".into()),
            max_words: Some(180),
            _score_threshold: None,
            collection: Some("workspace".into()),
        };

        let validated = validate_summarize_request(request, true).expect("validated");
        assert_eq!(validated.project_id.as_deref(), Some("default"));
        assert_eq!(validated.memory_type.as_deref(), Some("episodic"));
        assert_eq!(validated.limit, 20);
        assert_eq!(validated.max_words, 180);
        assert!(matches!(validated.strategy, SummarizeStrategy::Auto));
    }

    #[test]
    fn validate_summarize_request_rejects_invalid_strategy() {
        ensure_test_config();
        let request = SummarizeToolRequest {
            project_id: None,
            memory_type: None,
            tags: None,
            time_range: SummarizeToolTimeRange {
                start: Some("2025-01-01T00:00:00Z".into()),
                end: Some("2025-01-02T00:00:00Z".into()),
            },
            limit: None,
            strategy: Some("invalid".into()),
            provider: None,
            model: None,
            max_words: None,
            _score_threshold: None,
            collection: None,
        };

        let error = validate_summarize_request(request, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn validate_summarize_request_rejects_missing_end() {
        ensure_test_config();
        let request = SummarizeToolRequest {
            project_id: None,
            memory_type: None,
            tags: None,
            time_range: SummarizeToolTimeRange {
                start: Some("2025-01-01T00:00:00Z".into()),
                end: None,
            },
            limit: None,
            strategy: None,
            provider: None,
            model: None,
            max_words: None,
            _score_threshold: None,
            collection: None,
        };

        let error = validate_summarize_request(request, false).unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
