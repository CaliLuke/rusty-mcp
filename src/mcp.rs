use std::{borrow::Cow, sync::Arc};

use crate::{
    config::{EmbeddingProvider, get_config},
    processing::{
        IngestMetadata, ProcessingService, QdrantHealthSnapshot, SearchRequest, SearchTimeRange,
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

/// MCP server implementation exposing Rusty Memory operations.
#[derive(Clone)]
pub struct RustyMemMcpServer {
    processing: Arc<ProcessingService>,
}

const MEMORY_TYPES_URI: &str = "mcp://rusty-mem/memory-types";
const HEALTH_URI: &str = "mcp://rusty-mem/health";
const PROJECTS_URI: &str = "mcp://rusty-mem/projects";
const PROJECT_TAGS_TEMPLATE_URI: &str = "mcp://rusty-mem/projects/{project_id}/tags";
const PROJECT_TAGS_PREFIX: &str = "mcp://rusty-mem/projects/";
const PROJECT_TAGS_SUFFIX: &str = "/tags";

const SEARCH_DEFAULT_LIMIT: usize = 5;
const SEARCH_DEFAULT_THRESHOLD: f32 = 0.25;

impl RustyMemMcpServer {
    /// Create a new MCP server using the supplied processing pipeline.
    pub fn new(processing: Arc<ProcessingService>) -> Self {
        Self { processing }
    }

    fn describe_tools(&self) -> Vec<Tool> {
        let push_schema = Arc::new(index_input_schema());
        let search_schema = Arc::new(search_input_schema());
        vec![
            Tool {
                name: Cow::Borrowed("search"),
                title: Some("Search Memories".to_string()),
                description: Some(Cow::Owned(
                    "Search stored memories using semantic similarity. Provide `query_text` and optional filters (`project_id`, `memory_type`, `tags`, `time_range`, `collection`). Defaults: `limit=5`, `score_threshold=0.25`. Example: {\n  \"query_text\": \"current architecture plan\",\n  \"project_id\": \"default\",\n  \"tags\": [\"architecture\"],\n  \"limit\": 5\n}.".to_string(),
                )),
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
        memory_types.mime_type = Some("text".into());

        let mut health = RawResource::new(HEALTH_URI, "health");
        health.description = Some("Live embedding configuration and Qdrant reachability".into());
        health.mime_type = Some("text".into());

        let mut projects = RawResource::new(PROJECTS_URI, "projects");
        projects.description = Some("Distinct project_id values currently stored in Qdrant".into());
        projects.mime_type = Some("text".into());

        vec![
            memory_types.no_annotation(),
            health.no_annotation(),
            projects.no_annotation(),
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
            mime_type: Some("text".into()),
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
                "Rusty Memory MCP usage:\n1. Call listResources({}) to discover read-only resources, then readResource each URI (memory-types, health, projects).\n2. For tags, call listResourceTemplates({}) to obtain URI patterns such as mcp://rusty-mem/projects/{project_id}/tags, substitute the project_id, and invoke readResource.\n3. Call get-collections to discover existing Qdrant collections (pass {}).\n4. To create or update a collection, call new-collection with { \"name\": <collection>, \"vector_size\": <optional dimension> }.\n5. Index content with push using { \"text\": <document>, \"collection\": <optional override>, \"project_id\": \"default\" (optional), \"memory_type\": \"semantic\" (optional), \"tags\": [<strings>], \"source_uri\": <string> }. Responses include `chunksIndexed`, `chunkSize`, `inserted`, `updated`, and `skippedDuplicates`.\n6. Search memories with search using { \"query_text\": <question>, \"project_id\": \"default\" (optional), \"limit\": 5, \"score_threshold\": 0.25 }. Results return `id`, `score`, `text`, and stored metadata.\n7. Inspect ingestion counters via metrics (pass {}) to confirm documents were processed; the result echoes `lastChunkSize`.\nAll tool responses return structured JSON; prefer the structured payload over the text summary.".into(),
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
                    contents: vec![ResourceContents::text(
                        memory_types_payload(),
                        MEMORY_TYPES_URI,
                    )],
                }),
                HEALTH_URI => {
                    let config = get_config();
                    let snapshot = processing.qdrant_health().await;
                    Ok(ReadResourceResult {
                        contents: vec![ResourceContents::text(
                            health_payload(
                                config.embedding_provider,
                                &config.embedding_model,
                                config.embedding_dimension,
                                &config.qdrant_url,
                                &config.qdrant_collection_name,
                                &snapshot,
                            ),
                            HEALTH_URI,
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
                        contents: vec![ResourceContents::text(
                            serialize_json(&payload, PROJECTS_URI),
                            PROJECTS_URI,
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
                        contents: vec![ResourceContents::text(
                            serialize_json(&payload, other),
                            other,
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
                    let args: SearchToolRequest = parse_arguments(request.arguments)?;
                    if args.query_text.trim().is_empty() {
                        return Err(McpError::invalid_params(
                            "`query_text` must not be empty",
                            None,
                        ));
                    }

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

                    let config = get_config();
                    let collection_name = collection
                        .clone()
                        .unwrap_or_else(|| config.qdrant_collection_name.clone());
                    let limit_value = limit.unwrap_or(SEARCH_DEFAULT_LIMIT).max(1);
                    let threshold_value = score_threshold.unwrap_or(SEARCH_DEFAULT_THRESHOLD);

                    let search_request = SearchRequest {
                        query_text,
                        collection: Some(collection_name.clone()),
                        project_id,
                        memory_type,
                        tags,
                        time_range: time_range.map(SearchTimeRange::from),
                        limit: Some(limit_value),
                        score_threshold: Some(threshold_value),
                    };

                    let hits = processing
                        .search_memories(search_request)
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;

                    let results: Vec<Value> = hits
                        .into_iter()
                        .map(|hit| {
                            let mut item = Map::new();
                            item.insert("id".into(), Value::String(hit.id));
                            item.insert("score".into(), json!(hit.score));
                            if let Some(text) = hit.text {
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
                            Value::Object(item)
                        })
                        .collect();

                    Ok(CallToolResult::structured(json!({
                        "results": results,
                        "collection": collection_name,
                        "limit": limit_value,
                        "scoreThreshold": threshold_value,
                    })))
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchToolTimeRange {
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
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
    serde_json::from_value(value)
        .map_err(|err| McpError::invalid_params(format!("Invalid arguments: {err}"), None))
}

fn serialize_json<T: Serialize>(value: &T, context_uri: &str) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| {
        tracing::warn!(uri = context_uri, %error, "Failed to serialize JSON prettily");
        serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
    })
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
        Value::Number(serde_json::Number::from(SEARCH_DEFAULT_LIMIT as u64)),
    );
    properties.insert("limit".into(), Value::Object(limit_schema));

    let mut threshold_schema = JsonObject::new();
    threshold_schema.insert("type".into(), Value::String("number".into()));
    threshold_schema.insert(
        "description".into(),
        Value::String("Minimum score threshold for matches".into()),
    );
    threshold_schema.insert(
        "default".into(),
        Value::Number(
            serde_json::Number::from_f64(SEARCH_DEFAULT_THRESHOLD as f64)
                .expect("valid score threshold"),
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

    finalize_object_schema(properties, &["query_text"])
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
    use serde_json::Value;

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
