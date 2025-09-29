//! JSON schema builders for MCP tools.

use crate::config::get_config;
use serde_json::{Map, Value, json};

/// Build the schema describing the `push` tool input.
pub(crate) fn index_input_schema() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("text".into(), string_schema("Document contents to index"));

    let mut collection_schema = Map::new();
    collection_schema.insert("type".into(), Value::String("string".into()));
    collection_schema.insert(
        "description".into(),
        Value::String("Optional override for the Qdrant collection".into()),
    );
    properties.insert("collection".into(), Value::Object(collection_schema));

    let mut project_schema = Map::new();
    project_schema.insert("type".into(), Value::String("string".into()));
    project_schema.insert(
        "description".into(),
        Value::String("Optional project identifier; defaults to 'default'.".into()),
    );
    project_schema.insert("default".into(), Value::String("default".into()));
    properties.insert("project_id".into(), Value::Object(project_schema));

    let mut memory_schema = Map::new();
    memory_schema.insert("type".into(), Value::String("string".into()));
    memory_schema.insert(
        "description".into(),
        Value::String("Optional memory type; defaults to 'semantic'.".into()),
    );
    memory_schema.insert(
        "enum".into(),
        Value::Array(
            ["episodic", "semantic", "procedural"]
                .into_iter()
                .map(|variant| Value::String(variant.into()))
                .collect(),
        ),
    );
    memory_schema.insert("default".into(), Value::String("semantic".into()));
    properties.insert("memory_type".into(), Value::Object(memory_schema));

    let mut tag_item_schema = Map::new();
    tag_item_schema.insert("type".into(), Value::String("string".into()));
    let mut tags_schema = Map::new();
    tags_schema.insert("type".into(), Value::String("array".into()));
    tags_schema.insert(
        "description".into(),
        Value::String("Optional tags applied to each chunk.".into()),
    );
    tags_schema.insert("items".into(), Value::Object(tag_item_schema));
    properties.insert("tags".into(), Value::Object(tags_schema));

    let mut source_schema = Map::new();
    source_schema.insert("type".into(), Value::String("string".into()));
    source_schema.insert(
        "description".into(),
        Value::String("Optional URI (file path, URL) describing the memory source.".into()),
    );
    properties.insert("source_uri".into(), Value::Object(source_schema));

    finalize_object_schema(properties, &["text"])
}

/// Build the schema describing the `new-collection` tool input.
pub(crate) fn create_collection_input_schema() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("name".into(), string_schema("Collection name"));
    let mut vector_schema = Map::new();
    vector_schema.insert("type".into(), Value::String("integer".into()));
    vector_schema.insert(
        "description".into(),
        Value::String("Vector dimension (defaults to EMBEDDING_DIMENSION)".into()),
    );
    properties.insert("vector_size".into(), Value::Object(vector_schema));

    finalize_object_schema(properties, &["name"])
}

/// Build the schema describing the `search` tool input.
pub(crate) fn search_input_schema() -> Map<String, Value> {
    let config = get_config();
    let default_limit = config.search_default_limit;
    let max_limit = config.search_max_limit;
    let default_threshold = config.search_default_score_threshold;

    let mut properties = Map::new();
    properties.insert(
        "query_text".into(),
        string_schema("Natural language query text to embed and search with"),
    );

    let mut project_schema = Map::new();
    project_schema.insert("type".into(), Value::String("string".into()));
    project_schema.insert(
        "description".into(),
        Value::String("Filter results to a specific project_id".into()),
    );
    project_schema.insert("default".into(), Value::String("default".into()));
    properties.insert("project_id".into(), Value::Object(project_schema));

    let mut memory_schema = Map::new();
    memory_schema.insert("type".into(), Value::String("string".into()));
    memory_schema.insert(
        "description".into(),
        Value::String("Filter results to a specific memory_type".into()),
    );
    memory_schema.insert(
        "enum".into(),
        Value::Array(
            ["episodic", "semantic", "procedural"]
                .into_iter()
                .map(|variant| Value::String(variant.into()))
                .collect(),
        ),
    );
    properties.insert("memory_type".into(), Value::Object(memory_schema));

    let mut tag_item_schema = Map::new();
    tag_item_schema.insert("type".into(), Value::String("string".into()));
    let mut tags_schema = Map::new();
    tags_schema.insert("type".into(), Value::String("array".into()));
    tags_schema.insert(
        "description".into(),
        Value::String("Contains-any filter applied to payload tags".into()),
    );
    tags_schema.insert("items".into(), Value::Object(tag_item_schema));
    properties.insert("tags".into(), Value::Object(tags_schema));

    let mut time_range_properties = Map::new();
    time_range_properties.insert(
        "start".into(),
        string_schema("Inclusive RFC3339 timestamp lower bound"),
    );
    time_range_properties.insert(
        "end".into(),
        string_schema("Inclusive RFC3339 timestamp upper bound"),
    );
    let mut time_range_schema = Map::new();
    time_range_schema.insert("type".into(), Value::String("object".into()));
    time_range_schema.insert("properties".into(), Value::Object(time_range_properties));
    time_range_schema.insert("additionalProperties".into(), Value::Bool(false));
    properties.insert("time_range".into(), Value::Object(time_range_schema));

    let mut limit_schema = Map::new();
    limit_schema.insert("type".into(), Value::String("integer".into()));
    limit_schema.insert(
        "description".into(),
        Value::String("Maximum number of results to return".into()),
    );
    limit_schema.insert("minimum".into(), Value::Number(1.into()));
    limit_schema.insert(
        "default".into(),
        Value::Number(serde_json::Number::from(default_limit as u64)),
    );
    limit_schema.insert(
        "maximum".into(),
        Value::Number(serde_json::Number::from(max_limit as u64)),
    );
    properties.insert("limit".into(), Value::Object(limit_schema));

    let mut threshold_schema = Map::new();
    threshold_schema.insert("type".into(), Value::String("number".into()));
    threshold_schema.insert(
        "description".into(),
        Value::String("Minimum score threshold for matches".into()),
    );
    threshold_schema.insert(
        "minimum".into(),
        Value::Number(serde_json::Number::from_f64(0.0).expect("zero")),
    );
    threshold_schema.insert(
        "maximum".into(),
        Value::Number(serde_json::Number::from_f64(1.0).expect("one")),
    );
    threshold_schema.insert(
        "default".into(),
        Value::Number(
            serde_json::Number::from_f64(default_threshold as f64).expect("valid score threshold"),
        ),
    );
    properties.insert("score_threshold".into(), Value::Object(threshold_schema));

    let mut collection_schema = Map::new();
    collection_schema.insert("type".into(), Value::String("string".into()));
    collection_schema.insert(
        "description".into(),
        Value::String("Optional collection override".into()),
    );
    properties.insert("collection".into(), Value::Object(collection_schema));

    let mut schema = finalize_object_schema(properties, &["query_text"]);

    let example_canonical = json!({
        "query_text": "current architecture plan",
        "project_id": "default",
        "memory_type": "semantic",
        "tags": ["architecture"],
        "limit": 5
    });

    let example_aliases = json!({
        "query_text": "error budget policy",
        "project": "site",
        "type": "procedural",
        "tags": "sre",
        "k": 3
    });

    schema.insert(
        "examples".into(),
        Value::Array(vec![example_canonical, example_aliases]),
    );

    schema
}

/// Schema representing an empty object (used for parameterless tools).
pub(crate) fn empty_object_schema() -> Map<String, Value> {
    finalize_object_schema(Map::new(), &[])
}

/// Build the schema describing the `summarize` tool input.
pub(crate) fn summarize_input_schema() -> Map<String, Value> {
    let config = get_config();
    let max_limit = config.search_max_limit;

    let mut properties = Map::new();

    let mut project_schema = Map::new();
    project_schema.insert("type".into(), Value::String("string".into()));
    project_schema.insert(
        "description".into(),
        Value::String("Optional project filter; defaults to 'default' when omitted".into()),
    );
    project_schema.insert("default".into(), Value::String("default".into()));
    properties.insert("project_id".into(), Value::Object(project_schema));

    let mut memory_schema = Map::new();
    memory_schema.insert("type".into(), Value::String("string".into()));
    memory_schema.insert(
        "description".into(),
        Value::String("Memory type to summarize (default: 'episodic')".into()),
    );
    memory_schema.insert(
        "enum".into(),
        Value::Array(
            ["episodic", "semantic", "procedural"]
                .into_iter()
                .map(|v| Value::String(v.into()))
                .collect(),
        ),
    );
    memory_schema.insert("default".into(), Value::String("episodic".into()));
    properties.insert("memory_type".into(), Value::Object(memory_schema));

    let mut tag_item_schema = Map::new();
    tag_item_schema.insert("type".into(), Value::String("string".into()));
    let mut tags_schema = Map::new();
    tags_schema.insert("type".into(), Value::String("array".into()));
    tags_schema.insert(
        "description".into(),
        Value::String("Optional contains-any tag filter applied to episodic memories".into()),
    );
    tags_schema.insert("items".into(), Value::Object(tag_item_schema));
    properties.insert("tags".into(), Value::Object(tags_schema));

    let mut time_range_properties = Map::new();
    time_range_properties.insert(
        "start".into(),
        string_schema("Inclusive RFC3339 start timestamp"),
    );
    time_range_properties.insert(
        "end".into(),
        string_schema("Inclusive RFC3339 end timestamp"),
    );
    let mut time_range_schema = Map::new();
    time_range_schema.insert("type".into(), Value::String("object".into()));
    time_range_schema.insert("properties".into(), Value::Object(time_range_properties));
    time_range_schema.insert("additionalProperties".into(), Value::Bool(false));
    properties.insert("time_range".into(), Value::Object(time_range_schema));

    let mut limit_schema = Map::new();
    limit_schema.insert("type".into(), Value::String("integer".into()));
    limit_schema.insert(
        "description".into(),
        Value::String("Maximum episodic memories to include in the summarization prompt".into()),
    );
    limit_schema.insert("minimum".into(), Value::Number(1.into()));
    limit_schema.insert(
        "maximum".into(),
        Value::Number(serde_json::Number::from(max_limit as u64)),
    );
    limit_schema.insert("default".into(), Value::Number(50.into()));
    properties.insert("limit".into(), Value::Object(limit_schema));

    let mut strategy_schema = Map::new();
    strategy_schema.insert("type".into(), Value::String("string".into()));
    strategy_schema.insert(
        "enum".into(),
        Value::Array(
            ["auto", "abstractive", "extractive"]
                .into_iter()
                .map(|v| Value::String(v.into()))
                .collect(),
        ),
    );
    strategy_schema.insert("default".into(), Value::String("auto".into()));
    properties.insert("strategy".into(), Value::Object(strategy_schema));

    let mut provider_schema = Map::new();
    provider_schema.insert("type".into(), Value::String("string".into()));
    provider_schema.insert(
        "enum".into(),
        Value::Array(vec![
            Value::String("ollama".into()),
            Value::String("none".into()),
        ]),
    );
    properties.insert("provider".into(), Value::Object(provider_schema));

    properties.insert(
        "model".into(),
        string_schema("Optional override for the summarization model"),
    );

    let mut max_words_schema = Map::new();
    max_words_schema.insert("type".into(), Value::String("integer".into()));
    max_words_schema.insert(
        "description".into(),
        Value::String("Word budget for the final summary (must be > 0)".into()),
    );
    max_words_schema.insert("minimum".into(), Value::Number(1.into()));
    properties.insert("max_words".into(), Value::Object(max_words_schema));

    properties.insert(
        "collection".into(),
        string_schema("Optional collection override"),
    );

    finalize_object_schema(properties, &["time_range"])
}

fn string_schema(description: &str) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("string".into()));
    schema.insert("description".into(), Value::String(description.into()));
    Value::Object(schema)
}

fn finalize_object_schema(properties: Map<String, Value>, required: &[&str]) -> Map<String, Value> {
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert(
            "required".into(),
            Value::Array(
                required
                    .iter()
                    .map(|&key| Value::String(key.into()))
                    .collect(),
            ),
        );
    }
    schema.insert("additionalProperties".into(), Value::Bool(false));
    schema
}
