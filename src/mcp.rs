use std::{borrow::Cow, collections::HashSet, sync::Arc};

use crate::{
    config::{EmbeddingProvider, get_config},
    processing::{
        IngestMetadata, ProcessingService, QdrantHealthSnapshot, SearchError, SearchHit,
        SearchRequest, SearchTimeRange,
    },
};
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParam, CallToolResult, JsonObject,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, RawResource,
        RawResourceTemplate, ReadResourceRequestParam, ReadResourceResult, Resource,
        ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// MCP server implementation exposing Rusty Memory operations.
#[derive(Clone)]
pub struct RustyMemMcpServer {
    processing: Arc<ProcessingService>,
}

const MEMORY_TYPES_URI: &str = "mcp://rusty-mem/memory-types";
const HEALTH_URI: &str = "mcp://rusty-mem/health";
const PROJECTS_URI: &str = "mcp://rusty-mem/projects";
const SETTINGS_URI: &str = "mcp://rusty-mem/settings";
const PROJECT_TAGS_TEMPLATE_URI: &str = "mcp://rusty-mem/projects/{project_id}/tags";
const PROJECT_TAGS_PREFIX: &str = "mcp://rusty-mem/projects/";
const PROJECT_TAGS_SUFFIX: &str = "/tags";
const MEMORY_TYPES: [&str; 3] = ["episodic", "semantic", "procedural"];
const APPLICATION_JSON: &str = "application/json";

impl RustyMemMcpServer {
    /// Create a new MCP server using the supplied processing pipeline.
    pub fn new(processing: Arc<ProcessingService>) -> Self {
        Self { processing }
    }

    fn describe_tools(&self) -> Vec<Tool> {
        let push_schema = Arc::new(index_input_schema());
        let search_schema = Arc::new(search_input_schema());
        let config = get_config();
        let default_limit = config.search_default_limit;
        let default_threshold = config.search_default_score_threshold;
        let search_description = format!(
            "Search stored memories using semantic similarity. Provide `query_text` plus optional filters (`project_id`/`project`, `memory_type`/`type`, `tags`, `time_range`, `collection`, `limit`/`k`). Defaults: `limit={default_limit}`, `score_threshold={default_threshold}`. Response returns `results`, prompt-ready `context`, and a `used_filters` echo. Example: {{\\n  \\\"query_text\\\": \\\"current architecture plan\\\",\\n  \\\"project_id\\\": \\\"default\\\",\\n  \\\"tags\\\": [\\\"architecture\\\"],\\n  \\\"limit\\\": {default_limit}\\n}}."
        );
        vec![
            Tool {
                name: Cow::Borrowed("search"),
                title: Some("Search Memories".to_string()),
                description: Some(Cow::Owned(search_description)),
                input_schema: search_schema.clone(),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Search Memories")
                        .read_only(true)
                        .idempotent(true)
                        .open_world(false),
                ),
                icons: None,
            },
            Tool {
                name: Cow::Borrowed("push"),
                title: Some("Index Document".to_string()),
                description: Some(Cow::Borrowed(
                    "Split the provided text, embed each chunk, and upsert into Qdrant. Required: `text`. Optional metadata: `collection`, `project_id`, `memory_type`, `tags`, `source_uri`. Defaults: `project_id=default`, `memory_type=semantic`. The response echoes `chunksIndexed`, `chunkSize`, `inserted`, `updated`, and `skippedDuplicates`.",
                )),
                input_schema: push_schema.clone(),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Index Document")
                        .destructive(true)
                        .idempotent(false)
                        .open_world(false),
                ),
                icons: None,
            },
            Tool {
                name: Cow::Borrowed("get-collections"),
                title: Some("List Collections".to_string()),
                description: Some(Cow::Borrowed(
                    "Return the Qdrant collection names known to this server. No parameters: pass {}.",
                )),
                input_schema: Arc::new(empty_object_schema()),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("List Collections")
                        .read_only(true)
                        .idempotent(true)
                        .open_world(false),
                ),
                icons: None,
            },
            Tool {
                name: Cow::Borrowed("new-collection"),
                title: Some("Create Collection".to_string()),
                description: Some(Cow::Borrowed(
                    "Ensure a Qdrant collection exists with the desired vector size. Required: `name` (string). Optional: `vector_size` (integer) overrides the server default. Example: {\n  \"name\": \"support-notes\",\n  \"vector_size\": 1536\n}.",
                )),
                input_schema: Arc::new(create_collection_input_schema()),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Create Collection")
                        .destructive(false)
                        .idempotent(true)
                        .open_world(false),
                ),
                icons: None,
            },
            Tool {
                name: Cow::Borrowed("metrics"),
                title: Some("Metrics Snapshot".to_string()),
                description: Some(Cow::Borrowed(
                    "Fetch ingestion counters for documents and chunks processed so far. No parameters: send {}.",
                )),
                input_schema: Arc::new(empty_object_schema()),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Metrics Snapshot")
                        .read_only(true)
                        .idempotent(true)
                        .open_world(false),
                ),
                icons: None,
            },
        ]
    }

    fn describe_resources(&self) -> Vec<Resource> {
        let mut memory_types = RawResource::new(MEMORY_TYPES_URI, "memory-types");
        memory_types.description =
            Some("Supported memory_type values and default selection".into());
        memory_types.mime_type = Some(APPLICATION_JSON.into());

        let mut health = RawResource::new(HEALTH_URI, "health");
        health.description = Some("Live embedding configuration and Qdrant reachability".into());
        health.mime_type = Some(APPLICATION_JSON.into());

        let mut projects = RawResource::new(PROJECTS_URI, "projects");
        projects.description = Some("Distinct project_id values currently stored in Qdrant".into());
        projects.mime_type = Some(APPLICATION_JSON.into());

        let mut settings = RawResource::new(SETTINGS_URI, "settings");
        settings.description = Some("Effective defaults for search ergonomics".into());
        settings.mime_type = Some(APPLICATION_JSON.into());

        vec![
            memory_types.no_annotation(),
            health.no_annotation(),
            projects.no_annotation(),
            settings.no_annotation(),
        ]
    }

    fn describe_resource_templates(&self) -> Vec<ResourceTemplate> {
        let tags_template = RawResourceTemplate {
            uri_template: PROJECT_TAGS_TEMPLATE_URI.into(),
            name: "project-tags".into(),
            title: Some("Project Tags".into()),
            description: Some(
                "Enumerate distinct tags for a specific project: replace {project_id} and call readResource"
                    .into(),
            ),
            mime_type: Some(APPLICATION_JSON.into()),
        };

        vec![tags_template.no_annotation()]
    }
}

impl ServerHandler for RustyMemMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut implementation = rmcp::model::Implementation::from_build_env();
        implementation.name = "rusty-mem".to_string();
        implementation.title = Some("Rusty Memory MCP".to_string());
        implementation.version = env!("CARGO_PKG_VERSION").to_string();

        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: implementation,
            instructions: Some(
                "Rusty Memory MCP\n  1) listResources({}) → readResource URIs (memory-types, health, projects)\n  2) listResourceTemplates({}) → fill mcp://rusty-mem/projects/{project_id}/tags\n  3) get-collections({}) → discover Qdrant collections\n  4) new-collection({ name, vector_size? }) → ensure collection/vector size\n  5) push({ text, project_id?, memory_type?, tags?, source_uri?, collection? })\n  6) search({ query_text, project_id?, memory_type?, tags?, time_range?, limit?, score_threshold?, collection? })\n  Invalid inputs return `invalid_params` with a short fix; all responses are structured JSON.".into(),
            ),
            ..ServerInfo::default()
        }
    }

    fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        let resources = self.describe_resources();
        std::future::ready(Ok(ListResourcesResult::with_all_items(resources)))
    }

    fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_
    {
        let templates = self.describe_resource_templates();
        std::future::ready(Ok(ListResourceTemplatesResult::with_all_items(templates)))
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.describe_tools();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        let processing = self.processing.clone();
        async move {
            match request.uri.as_str() {
                MEMORY_TYPES_URI => Ok(ReadResourceResult {
                    contents: vec![json_resource_contents(
                        MEMORY_TYPES_URI,
                        memory_types_payload(),
                    )],
                }),
                HEALTH_URI => {
                    let config = get_config();
                    let snapshot = processing.qdrant_health().await;
                    Ok(ReadResourceResult {
                        contents: vec![json_resource_contents(
                            HEALTH_URI,
                            health_payload(
                                config.embedding_provider,
                                &config.embedding_model,
                                config.embedding_dimension,
                                &config.qdrant_url,
                                &config.qdrant_collection_name,
                                &snapshot,
                            ),
                        )],
                    })
                }
                PROJECTS_URI => {
                    let config = get_config();
                    let projects = processing
                        .list_projects(&config.qdrant_collection_name)
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    let payload = ProjectsSnapshot {
                        projects: projects.into_iter().collect(),
                    };
                    Ok(ReadResourceResult {
                        contents: vec![json_resource_contents(
                            PROJECTS_URI,
                            serialize_json(&payload, PROJECTS_URI),
                        )],
                    })
                }
                SETTINGS_URI => {
                    let config = get_config();
                    let payload = SettingsSnapshot {
                        search: SearchSettingsSnapshot {
                            default_limit: config.search_default_limit,
                            max_limit: config.search_max_limit,
                            default_score_threshold: config.search_default_score_threshold,
                        },
                    };
                    Ok(ReadResourceResult {
                        contents: vec![json_resource_contents(
                            SETTINGS_URI,
                            serialize_json(&payload, SETTINGS_URI),
                        )],
                    })
                }
                other
                    if other.starts_with(PROJECT_TAGS_PREFIX)
                        && other.ends_with(PROJECT_TAGS_SUFFIX) =>
                {
                    let project_segment =
                        &other[PROJECT_TAGS_PREFIX.len()..other.len() - PROJECT_TAGS_SUFFIX.len()];
                    if project_segment.is_empty() {
                        return Err(McpError::invalid_params(
                            "Project identifier missing in resource URI",
                            None,
                        ));
                    }
                    let config = get_config();
                    let tags = processing
                        .list_tags(&config.qdrant_collection_name, Some(project_segment))
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    let payload = ProjectTagsSnapshot {
                        project_id: project_segment.to_string(),
                        tags: tags.into_iter().collect(),
                    };
                    Ok(ReadResourceResult {
                        contents: vec![json_resource_contents(
                            other,
                            serialize_json(&payload, other),
                        )],
                    })
                }
                other => Err(McpError::invalid_params(
                    format!("Unknown resource URI: {other}"),
                    None,
                )),
            }
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let processing = self.processing.clone();
        async move {
            match request.name.as_ref() {
                "push" => {
                    let args: IndexToolRequest = parse_arguments(request.arguments)?;
                    if args.text.trim().is_empty() {
                        return Err(McpError::invalid_params("`text` must not be empty", None));
                    }
                    let IndexToolRequest {
                        text,
                        collection,
                        project_id,
                        memory_type,
                        tags,
                        source_uri,
                    } = args;
                    let collection =
                        collection.unwrap_or_else(|| get_config().qdrant_collection_name.clone());
                    let metadata = IngestMetadata {
                        project_id,
                        memory_type,
                        tags,
                        source_uri,
                    };
                    let outcome = processing
                        .process_and_index(&collection, text, metadata)
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    Ok(CallToolResult::structured(json!({
                        "status": "ok",
                        "collection": collection,
                        "chunksIndexed": outcome.chunk_count,
                        "chunkSize": outcome.chunk_size,
                        "inserted": outcome.inserted,
                        "updated": outcome.updated,
                        "skippedDuplicates": outcome.skipped_duplicates,
                    })))
                }
                "search" => {
                    let normalized_arguments = normalize_search_arguments(request.arguments);
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
                "get-collections" => {
                    let collections = processing
                        .list_collections()
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    Ok(CallToolResult::structured(json!({
                        "collections": collections,
                    })))
                }
                "new-collection" => {
                    let args: CreateCollectionRequest = parse_arguments(request.arguments)?;
                    if args.name.trim().is_empty() {
                        return Err(McpError::invalid_params("`name` must not be empty", None));
                    }
                    let target_size = args.vector_size.unwrap_or_else(|| {
                        let cfg = get_config();
                        cfg.embedding_dimension as u64
                    });
                    processing
                        .create_collection(&args.name, Some(target_size))
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    Ok(CallToolResult::structured(json!({
                        "status": "ok",
                        "vectorSize": target_size,
                    })))
                }
                "metrics" => {
                    let snapshot = processing.metrics_snapshot();
                    Ok(CallToolResult::structured(json!({
                        "documentsIndexed": snapshot.documents_indexed,
                        "chunksIndexed": snapshot.chunks_indexed,
                        "lastChunkSize": snapshot.last_chunk_size,
                    })))
                }
                other => Err(McpError::invalid_params(
                    format!("Unknown tool: {other}"),
                    None,
                )),
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct IndexToolRequest {
    text: String,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    source_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateCollectionRequest {
    name: String,
    #[serde(default)]
    vector_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchToolRequest {
    query_text: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    time_range: Option<SearchToolTimeRange>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    score_threshold: Option<f32>,
    #[serde(default)]
    collection: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct SearchToolTimeRange {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}

#[derive(Debug)]
struct ValidatedSearchInput {
    query_text: String,
    project_id: Option<String>,
    memory_type: Option<String>,
    tags: Option<Vec<String>>,
    time_range: Option<SearchToolTimeRange>,
    limit: usize,
    score_threshold: f32,
    collection: Option<String>,
}

impl From<SearchToolTimeRange> for SearchTimeRange {
    fn from(value: SearchToolTimeRange) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

fn parse_arguments<T: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, McpError> {
    let value = arguments
        .map(Value::Object)
        .unwrap_or_else(|| Value::Object(JsonObject::new()));
    parse_arguments_value(value)
}

fn parse_arguments_value<T: DeserializeOwned>(value: Value) -> Result<T, McpError> {
    serde_json::from_value(value)
        .map_err(|err| McpError::invalid_params(format!("Invalid arguments: {err}"), None))
}

fn normalize_search_arguments(arguments: Option<JsonObject>) -> Value {
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

fn format_search_hits(hits: Vec<SearchHit>) -> (Vec<Value>, Option<String>) {
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

fn build_search_response(
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

fn serialize_json<T: Serialize>(value: &T, context_uri: &str) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| {
        tracing::warn!(uri = context_uri, %error, "Failed to serialize JSON prettily");
        serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
    })
}

fn json_resource_contents(uri: &str, text: String) -> ResourceContents {
    ResourceContents::TextResourceContents {
        uri: uri.to_string(),
        mime_type: Some(APPLICATION_JSON.into()),
        text,
        meta: None,
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectsSnapshot {
    projects: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectTagsSnapshot {
    project_id: String,
    tags: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SettingsSnapshot {
    search: SearchSettingsSnapshot,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SearchSettingsSnapshot {
    default_limit: usize,
    max_limit: usize,
    default_score_threshold: f32,
}

fn memory_types_payload() -> String {
    serde_json::to_string_pretty(&json!({
        "memory_types": ["episodic", "semantic", "procedural"],
        "default": "semantic"
    }))
    .unwrap_or_else(|_| {
        "{\"memory_types\":[\"episodic\",\"semantic\",\"procedural\"],\"default\":\"semantic\"}"
            .into()
    })
}

fn health_payload(
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

fn index_input_schema() -> JsonObject {
    let mut properties = JsonObject::new();
    properties.insert("text".into(), string_schema("Document contents to index"));
    let mut collection_schema = JsonObject::new();
    collection_schema.insert("type".into(), Value::String("string".into()));
    collection_schema.insert(
        "description".into(),
        Value::String("Optional override for the Qdrant collection".into()),
    );
    properties.insert("collection".into(), Value::Object(collection_schema));

    let mut project_schema = JsonObject::new();
    project_schema.insert("type".into(), Value::String("string".into()));
    project_schema.insert(
        "description".into(),
        Value::String("Optional project identifier; defaults to 'default'.".into()),
    );
    project_schema.insert("default".into(), Value::String("default".into()));
    properties.insert("project_id".into(), Value::Object(project_schema));

    let mut memory_schema = JsonObject::new();
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

    let mut tag_item_schema = JsonObject::new();
    tag_item_schema.insert("type".into(), Value::String("string".into()));
    let mut tags_schema = JsonObject::new();
    tags_schema.insert("type".into(), Value::String("array".into()));
    tags_schema.insert(
        "description".into(),
        Value::String("Optional tags applied to each chunk.".into()),
    );
    tags_schema.insert("items".into(), Value::Object(tag_item_schema));
    properties.insert("tags".into(), Value::Object(tags_schema));

    let mut source_schema = JsonObject::new();
    source_schema.insert("type".into(), Value::String("string".into()));
    source_schema.insert(
        "description".into(),
        Value::String("Optional URI (file path, URL) describing the memory source.".into()),
    );
    properties.insert("source_uri".into(), Value::Object(source_schema));

    finalize_object_schema(properties, &["text"])
}

fn create_collection_input_schema() -> JsonObject {
    let mut properties = JsonObject::new();
    properties.insert("name".into(), string_schema("Collection name"));
    let mut vector_schema = JsonObject::new();
    vector_schema.insert("type".into(), Value::String("integer".into()));
    vector_schema.insert(
        "description".into(),
        Value::String("Vector dimension (defaults to EMBEDDING_DIMENSION)".into()),
    );
    properties.insert("vector_size".into(), Value::Object(vector_schema));

    finalize_object_schema(properties, &["name"])
}

fn search_input_schema() -> JsonObject {
    let config = get_config();
    let default_limit = config.search_default_limit;
    let max_limit = config.search_max_limit;
    let default_threshold = config.search_default_score_threshold;

    let mut properties = JsonObject::new();
    properties.insert(
        "query_text".into(),
        string_schema("Natural language query text to embed and search with"),
    );

    let mut project_schema = JsonObject::new();
    project_schema.insert("type".into(), Value::String("string".into()));
    project_schema.insert(
        "description".into(),
        Value::String("Filter results to a specific project_id".into()),
    );
    project_schema.insert("default".into(), Value::String("default".into()));
    properties.insert("project_id".into(), Value::Object(project_schema));

    let mut memory_schema = JsonObject::new();
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

    let mut tag_item_schema = JsonObject::new();
    tag_item_schema.insert("type".into(), Value::String("string".into()));
    let mut tags_schema = JsonObject::new();
    tags_schema.insert("type".into(), Value::String("array".into()));
    tags_schema.insert(
        "description".into(),
        Value::String("Contains-any filter applied to payload tags".into()),
    );
    tags_schema.insert("items".into(), Value::Object(tag_item_schema));
    properties.insert("tags".into(), Value::Object(tags_schema));

    let mut time_range_properties = JsonObject::new();
    time_range_properties.insert(
        "start".into(),
        string_schema("Inclusive RFC3339 timestamp lower bound"),
    );
    time_range_properties.insert(
        "end".into(),
        string_schema("Inclusive RFC3339 timestamp upper bound"),
    );
    let mut time_range_schema = JsonObject::new();
    time_range_schema.insert("type".into(), Value::String("object".into()));
    time_range_schema.insert("properties".into(), Value::Object(time_range_properties));
    time_range_schema.insert("additionalProperties".into(), Value::Bool(false));
    properties.insert("time_range".into(), Value::Object(time_range_schema));

    let mut limit_schema = JsonObject::new();
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

    let mut threshold_schema = JsonObject::new();
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

    let mut collection_schema = JsonObject::new();
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

fn empty_object_schema() -> JsonObject {
    finalize_object_schema(JsonObject::new(), &[])
}

fn string_schema(description: &str) -> Value {
    let mut schema = JsonObject::new();
    schema.insert("type".into(), Value::String("string".into()));
    schema.insert("description".into(), Value::String(description.into()));
    Value::Object(schema)
}

fn finalize_object_schema(properties: JsonObject, required: &[&str]) -> JsonObject {
    let mut schema = JsonObject::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONFIG, Config, EmbeddingProvider};
    use crate::processing::SearchHit;
    use rmcp::model::ErrorCode;
    use serde_json::{Map, Value};
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
    fn memory_types_payload_is_valid_json() {
        ensure_test_config();
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
            .expect("time_range object");
        assert_eq!(
            time_value.get("start"),
            Some(&Value::String("2024-01-01T00:00:00Z".into()))
        );
        assert!(time_value.get("end").is_none());
    }

    #[test]
    fn format_search_hits_builds_context_with_citations() {
        ensure_test_config();
        let hits = vec![
            SearchHit {
                id: "a1".into(),
                score: 0.9,
                text: Some("First snippet".into()),
                project_id: Some("alpha".into()),
                memory_type: None,
                tags: None,
                timestamp: None,
                source_uri: None,
            },
            SearchHit {
                id: "b2".into(),
                score: 0.7,
                text: Some("".into()),
                project_id: None,
                memory_type: None,
                tags: None,
                timestamp: None,
                source_uri: None,
            },
            SearchHit {
                id: "c3".into(),
                score: 0.8,
                text: Some("Second snippet".into()),
                project_id: None,
                memory_type: Some("semantic".into()),
                tags: Some(vec!["docs".into()]),
                timestamp: Some("2024-03-01T00:00:00Z".into()),
                source_uri: Some("file://note".into()),
            },
        ];

        let (results, context) = format_search_hits(hits);

        assert_eq!(results.len(), 3);
        assert_eq!(
            context,
            Some("First snippet [a1]\nSecond snippet [c3]".into())
        );

        let ids: Vec<&str> = results
            .iter()
            .map(|value| value["id"].as_str().expect("id string"))
            .collect();
        assert_eq!(ids, vec!["a1", "b2", "c3"]);
    }

    #[test]
    fn search_input_schema_includes_examples_and_defaults() {
        ensure_test_config();
        let schema = search_input_schema();
        assert_eq!(schema["properties"]["project_id"]["default"], "default");
        assert_eq!(schema["additionalProperties"], false);

        let memory_enum = schema["properties"]["memory_type"]["enum"].as_array();
        assert!(memory_enum.is_some());

        let examples = schema["examples"].as_array().expect("examples array");
        assert_eq!(examples.len(), 2);
        assert!(examples.iter().any(|value| value["project"] == "site"));
    }

    #[test]
    fn validate_search_request_rejects_empty_query() {
        ensure_test_config();
        let mut request = base_search_request();
        request.query_text = "   ".into();

        let err = validate_search_request(request, false, false).expect_err("should fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("query_text"));
    }

    #[test]
    fn validate_search_request_rejects_invalid_memory_type() {
        ensure_test_config();
        let mut request = base_search_request();
        request.memory_type = Some("unknown".into());

        let err = validate_search_request(request, false, false).expect_err("should fail");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("memory_type"));
    }

    #[test]
    fn validate_search_request_rejects_limit_out_of_bounds() {
        ensure_test_config();
        let mut request = base_search_request();
        request.limit = Some(0);
        let err = validate_search_request(request, false, false).expect_err("limit 0");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

        let mut request = base_search_request();
        request.limit = Some(999);
        let err = validate_search_request(request, false, false).expect_err("limit high");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("limit"));
    }

    #[test]
    fn validate_search_request_rejects_score_threshold_out_of_range() {
        ensure_test_config();
        let mut request = base_search_request();
        request.score_threshold = Some(-0.1);
        let err = validate_search_request(request, false, false).expect_err("negative");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

        let mut request = base_search_request();
        request.score_threshold = Some(1.1);
        let err = validate_search_request(request, false, false).expect_err("gt1");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("score_threshold"));
    }

    #[test]
    fn validate_search_request_rejects_empty_tags() {
        ensure_test_config();
        let mut request = base_search_request();
        request.tags = Some(vec!["".into()]);
        let err = validate_search_request(request, true, false).expect_err("empty tag");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("tags"));
    }

    #[test]
    fn parse_arguments_value_rejects_non_array_tags() {
        ensure_test_config();
        let mut raw = JsonObject::new();
        raw.insert("query_text".into(), Value::String("demo".into()));
        raw.insert("tags".into(), Value::Number(1.into()));

        let err = parse_arguments_value::<SearchToolRequest>(Value::Object(raw))
            .expect_err("tags must be array");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("Invalid arguments"));
    }

    #[test]
    fn validate_time_range_rejects_invalid_rfc3339() {
        ensure_test_config();
        let range = Some(SearchToolTimeRange {
            start: Some("not-a-time".into()),
            end: None,
        });
        let err = validate_time_range(range, true).expect_err("invalid start");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("time_range.start"));
    }

    #[test]
    fn validate_time_range_rejects_start_after_end() {
        ensure_test_config();
        let range = Some(SearchToolTimeRange {
            start: Some("2024-02-01T00:00:00Z".into()),
            end: Some("2024-01-01T00:00:00Z".into()),
        });
        let err = validate_time_range(range, true).expect_err("start> end");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("earlier"));
    }

    #[test]
    fn validate_time_range_requires_at_least_one_boundary_when_present() {
        ensure_test_config();
        let range = Some(SearchToolTimeRange {
            start: None,
            end: None,
        });
        let err = validate_time_range(range, true).expect_err("empty range");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn build_search_response_includes_threshold_fields() {
        ensure_test_config();
        let response = build_search_response("rusty".into(), 5, 0.4, vec![], None, Map::new());

        let score_threshold = response["score_threshold"].as_f64().expect("threshold");
        let legacy_threshold = response["scoreThreshold"]
            .as_f64()
            .expect("legacy threshold");
        assert!((score_threshold - 0.4).abs() < 1e-6);
        assert!((legacy_threshold - 0.4).abs() < 1e-6);
    }

    #[test]
    fn json_resource_contents_sets_application_json_mime() {
        ensure_test_config();
        let resource = json_resource_contents("test", "{}".into());
        match resource {
            ResourceContents::TextResourceContents { mime_type, .. } => {
                assert_eq!(mime_type.as_deref(), Some(APPLICATION_JSON));
            }
            _ => panic!("expected text resource"),
        }
    }

    #[test]
    fn map_search_error_wraps_embedding_errors() {
        ensure_test_config();
        let error = SearchError::DimensionMismatch {
            expected: 3,
            actual: 5,
        };
        let mapped = map_search_error(error);
        assert_eq!(mapped.code, ErrorCode::INTERNAL_ERROR);
        assert!(mapped.message.contains("Embedding dimension mismatch"));
    }
}
