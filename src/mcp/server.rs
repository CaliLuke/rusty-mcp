//! MCP server bootstrap and request dispatch.

use std::{borrow::Cow, sync::Arc};

use crate::{
    config::get_config,
    mcp::{
        format::{
            ProjectTagsSnapshot, ProjectsSnapshot, SearchSettingsSnapshot, SettingsSnapshot,
            health_payload, json_resource_contents, memory_types_payload, serialize_json,
        },
        handlers::{
            collections::{handle_create_collection, handle_list_collections},
            index::handle_push,
            metrics::handle_metrics,
            search::handle_search,
        },
        schemas,
    },
    processing::ProcessingService,
};
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParam, CallToolResult, ListResourceTemplatesResult,
        ListResourcesResult, ListToolsResult, RawResource, RawResourceTemplate,
        ReadResourceRequestParam, ReadResourceResult, Resource, ResourceTemplate,
        ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
};
const MEMORY_TYPES_URI: &str = "mcp://rusty-mem/memory-types";
const HEALTH_URI: &str = "mcp://rusty-mem/health";
const PROJECTS_URI: &str = "mcp://rusty-mem/projects";
const SETTINGS_URI: &str = "mcp://rusty-mem/settings";
const USAGE_URI: &str = "mcp://rusty-mem/usage";
const PROJECT_TAGS_TEMPLATE_URI: &str = "mcp://rusty-mem/projects/{project_id}/tags";
const PROJECT_TAGS_PREFIX: &str = "mcp://rusty-mem/projects/";
const PROJECT_TAGS_SUFFIX: &str = "/tags";

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
        let push_schema = Arc::new(schemas::index_input_schema());
        let search_schema = Arc::new(schemas::search_input_schema());
        let config = get_config();
        let default_limit = config.search_default_limit;
        let default_threshold = config.search_default_score_threshold;
        let search_description = format!(
            "Semantic search over indexed memories. Provide a short `query_text` plus optional filters (`project_id`/`project`, `memory_type`/`type`, `tags`, `time_range`, `collection`, `limit`/`k`).\n\nUsage policy: Do not paste large documents into prompts; index text with `push` first, then `search` to retrieve. Keep `query_text` concise (<=512 chars).\n\nDefaults: `limit={default_limit}`, `score_threshold={default_threshold}`. Response returns `results`, prompt-ready `context`, and `used_filters`. Example: {{\\n  \\\"query_text\\\": \\\"current architecture plan\\\",\\n  \\\"project_id\\\": \\\"default\\\",\\n  \\\"tags\\\": [\\\"architecture\\\"],\\n  \\\"limit\\\": {default_limit}\\n}}."
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
                    "Split the provided text, embed each chunk, and upsert into Qdrant. Required: `text`. Optional: `collection`, `project_id`, `memory_type`, `tags`, `source_uri`. Defaults: `project_id=default`, `memory_type=semantic`.\n\nBest practice: Use `push` to index source material, then call `search` (and `summarize` when available). Avoid concatenating full documents in chat—let the memory store handle retrieval. The response echoes `chunksIndexed`, `chunkSize`, `inserted`, `updated`, and `skippedDuplicates`.",
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
                input_schema: Arc::new(schemas::empty_object_schema()),
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
                input_schema: Arc::new(schemas::create_collection_input_schema()),
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
                input_schema: Arc::new(schemas::empty_object_schema()),
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

        let mut health = RawResource::new(HEALTH_URI, "health");
        health.description = Some("Live embedding configuration and Qdrant reachability".into());

        let mut projects = RawResource::new(PROJECTS_URI, "projects");
        projects.description = Some("Distinct project_id values currently stored in Qdrant".into());

        let mut settings = RawResource::new(SETTINGS_URI, "settings");
        settings.description = Some("Effective defaults for search ergonomics".into());

        let mut usage = RawResource::new(USAGE_URI, "usage");
        usage.description = Some(
            "Recommended tool flow and anti-patterns: use push→search→(summarize), avoid pasting long docs in prompts."
                .into(),
        );

        vec![
            memory_types.no_annotation(),
            health.no_annotation(),
            projects.no_annotation(),
            settings.no_annotation(),
            usage.no_annotation(),
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
            mime_type: Some(super::format::APPLICATION_JSON.into()),
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
                "Rusty Memory MCP\n  1) listResources({}) → readResource URIs (memory-types, health, projects, usage)\n  2) readResource('mcp://rusty-mem/usage') → usage policy & recommended flows\n  3) listResourceTemplates({}) → fill mcp://rusty-mem/projects/{project_id}/tags\n  4) get-collections({}) → discover Qdrant collections\n  5) new-collection({ name, vector_size? }) → ensure collection/vector size\n  6) push({ text, project_id?, memory_type?, tags?, source_uri?, collection? })\n  7) search({ query_text, project_id?, memory_type?, tags?, time_range?, limit?, score_threshold?, collection? })\n  Policy: do not paste large documents; index with push → search. Invalid inputs return `invalid_params` with a short fix; all responses are structured JSON.".into(),
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
                USAGE_URI => {
                    let usage = serde_json::json!({
                        "title": "Rusty Memory MCP Usage",
                        "policy": [
                            "Do not paste or concatenate large documents in prompts.",
                            "Index text with `push` first; use `search` to retrieve.",
                            "Prefer filters: project_id, memory_type, tags, time_range.",
                            "Keep query_text concise (<= 512 chars).",
                            "Use summarize (M9) to consolidate episodic → semantic summaries.",
                        ],
                        "flows": [
                            {
                                "name": "Ingest & Retrieve",
                                "steps": [
                                    "push({ text, project_id?, memory_type?, tags? })",
                                    "search({ query_text, project_id?, memory_type?, tags?, time_range? })"
                                ]
                            },
                            {
                                "name": "Summarize (M9)",
                                "steps": [
                                    "search episodic within time_range",
                                    "summarize({ project_id, time_range, tags?, limit?, max_words? })"
                                ]
                            }
                        ]
                    });
                    Ok(ReadResourceResult {
                        contents: vec![json_resource_contents(
                            USAGE_URI,
                            serialize_json(&usage, USAGE_URI),
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
                "push" => handle_push(&processing, request.arguments).await,
                "search" => handle_search(&processing, request.arguments).await,
                "get-collections" => handle_list_collections(&processing).await,
                "new-collection" => handle_create_collection(&processing, request.arguments).await,
                "metrics" => handle_metrics(&processing).await,
                other => Err(McpError::invalid_params(
                    format!("Unknown tool: {other}"),
                    None,
                )),
            }
        }
    }
}
