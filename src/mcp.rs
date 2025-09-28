use std::{borrow::Cow, sync::Arc};

use crate::{
    config::{EmbeddingProvider, get_config},
    processing::{IngestMetadata, ProcessingService, QdrantHealthSnapshot},
};
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParam, CallToolResult, JsonObject, ListResourcesResult,
        ListToolsResult, RawResource, ReadResourceRequestParam, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

/// MCP server implementation exposing Rusty Memory operations.
#[derive(Clone)]
pub struct RustyMemMcpServer {
    processing: Arc<ProcessingService>,
}

const MEMORY_TYPES_URI: &str = "mcp://rusty-mem/memory-types";
const HEALTH_URI: &str = "mcp://rusty-mem/health";

impl RustyMemMcpServer {
    /// Create a new MCP server using the supplied processing pipeline.
    pub fn new(processing: Arc<ProcessingService>) -> Self {
        Self { processing }
    }

    fn describe_tools(&self) -> Vec<Tool> {
        let push_schema = Arc::new(index_input_schema());
        vec![
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
                name: Cow::Borrowed("index"),
                title: Some("Alias: push".to_string()),
                description: Some(Cow::Borrowed(
                    "Alias for `push` to mirror the HTTP endpoint name. Accepts the same payload and returns identical results.",
                )),
                input_schema: push_schema.clone(),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Index Document (alias)")
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

        vec![memory_types.no_annotation(), health.no_annotation()]
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
                "Rusty Memory MCP usage:\n1. Call listResources({}) to discover read-only resources, then readResource each URI (e.g., mcp://rusty-mem/memory-types, mcp://rusty-mem/health) for supported metadata and health snapshots.\n2. Call get-collections to discover existing Qdrant collections (pass {}).\n3. To create or update a collection, call new-collection with { \"name\": <collection>, \"vector_size\": <optional dimension> }.\n4. Push content with push (or index) using { \"text\": <document>, \"collection\": <optional override>, \"project_id\": \"default\" (optional), \"memory_type\": \"semantic\" (optional), \"tags\": [<strings>], \"source_uri\": <string> }. Responses include `chunksIndexed`, `chunkSize`, `inserted`, `updated`, and `skippedDuplicates`.\n5. Inspect ingestion counters via metrics (pass {}) to confirm documents were processed; the result echoes `lastChunkSize`.\nAll tool responses return structured JSON; prefer the structured payload over the text summary.".into(),
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
                "push" | "index" => {
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

fn parse_arguments<T: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<T, McpError> {
    let value = arguments
        .map(Value::Object)
        .unwrap_or_else(|| Value::Object(JsonObject::new()));
    serde_json::from_value(value)
        .map_err(|err| McpError::invalid_params(format!("Invalid arguments: {err}"), None))
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
