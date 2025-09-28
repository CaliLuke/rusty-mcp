use std::{borrow::Cow, sync::Arc};

use crate::{config::get_config, processing::ProcessingService};
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, JsonObject, ListToolsResult, ServerCapabilities,
        ServerInfo, Tool, ToolAnnotations,
    },
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// MCP server implementation exposing Rusty Memory operations.
#[derive(Clone)]
pub struct RustyMemMcpServer {
    processing: Arc<ProcessingService>,
}

impl RustyMemMcpServer {
    /// Create a new MCP server using the supplied processing pipeline.
    pub fn new(processing: Arc<ProcessingService>) -> Self {
        Self { processing }
    }

    fn describe_tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: Cow::Borrowed("push"),
                title: Some("Index Document".to_string()),
                description: Some(Cow::Borrowed(
                    "Split the provided text, embed each chunk, and upsert into Qdrant. Required: `text` (string). Optional: `collection` (string) overrides the default target. Example: {\n  \"text\": \"docs about user onboarding\",\n  \"collection\": \"support-notes\"\n}.",
                )),
                input_schema: Arc::new(index_input_schema()),
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
}

impl ServerHandler for RustyMemMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut implementation = rmcp::model::Implementation::from_build_env();
        implementation.name = "rusty-mem".to_string();
        implementation.title = Some("Rusty Memory MCP".to_string());
        implementation.version = env!("CARGO_PKG_VERSION").to_string();

        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: implementation,
            instructions: Some(
                "Rusty Memory MCP usage:\n1. Call get-collections to discover existing Qdrant collections (pass {}).\n2. To create or update a collection, call new-collection with { \"name\": <collection>, \"vector_size\": <optional dimension> }.\n3. Push content with push using { \"text\": <document>, \"collection\": <optional override> }.\n4. Inspect ingestion counters via metrics (pass {}) to confirm documents were processed.\nAll tool responses return structured JSON; prefer the structured payload over the text summary.".into(),
            ),
            ..ServerInfo::default()
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.describe_tools();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
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
                    let collection = args
                        .collection
                        .unwrap_or_else(|| get_config().qdrant_collection_name.clone());
                    let text = args.text;
                    let chunks = processing
                        .process_and_index(&collection, text)
                        .await
                        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                    Ok(CallToolResult::structured(json!({
                        "status": "ok",
                        "collection": collection,
                        "chunksIndexed": chunks,
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
