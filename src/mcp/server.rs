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
            summarize::handle_summarize,
        },
        registry, schemas,
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
const MEMORY_TYPES_URI: &str = "mcp://memory-types";
const HEALTH_URI: &str = "mcp://health";
const PROJECTS_URI: &str = "mcp://projects";
const SETTINGS_URI: &str = "mcp://settings";
const USAGE_URI: &str = "mcp://usage";
const PROJECT_TAGS_TEMPLATE_URI: &str = "mcp://{project_id}/tags";
const PROJECT_TAGS_PREFIX: &str = "mcp://";
const PROJECT_TAGS_SUFFIX: &str = "/tags";

/// MCP server implementation exposing Rusty Memory operations.
#[derive(Clone)]
pub struct RustyMemMcpServer {
    processing: Arc<ProcessingService>,
    registry: Arc<registry::Registry>,
}

impl RustyMemMcpServer {
    /// Create a new MCP server using the supplied processing pipeline.
    pub fn new(processing: Arc<ProcessingService>) -> Self {
        let mut registry = registry::Registry::new();
        registry.register_resource(MEMORY_TYPES_URI, resource_memory_types);
        registry.register_resource(HEALTH_URI, resource_health);
        registry.register_resource(PROJECTS_URI, resource_projects);
        registry.register_resource(SETTINGS_URI, resource_settings);
        registry.register_resource(USAGE_URI, resource_usage);

        registry.register_tool("push", tool_push);
        registry.register_tool("search", tool_search);
        registry.register_tool("get-collections", tool_get_collections);
        registry.register_tool("new-collection", tool_new_collection);
        registry.register_tool("metrics", tool_metrics);
        registry.register_tool("summarize", tool_summarize);

        Self {
            processing,
            registry: Arc::new(registry),
        }
    }

    fn describe_tools(&self) -> Vec<Tool> {
        let push_schema = Arc::new(schemas::index_input_schema());
        let search_schema = Arc::new(schemas::search_input_schema());
        let summarize_schema = Arc::new(schemas::summarize_input_schema());
        vec![
            Tool {
                name: Cow::Borrowed("search"),
                title: Some("Search Memories".to_string()),
                description: Some(Cow::Borrowed(
                    "Retrieve the most relevant memories to ground your next step; add filters for project/type/tags/time.",
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
                    "Store source text as retrievable memory instead of pasting it into chats.",
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
                    "See which memory collections exist before you index or search.",
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
                title: Some("Create/Resize Collection".to_string()),
                description: Some(Cow::Borrowed(
                    "Create or resize a collection when starting a project or switching embedding dimensions.",
                )),
                input_schema: Arc::new(schemas::create_collection_input_schema()),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Create/Resize Collection")
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
                    "Check ingestion volume and last chunk size at a glance.",
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
            Tool {
                name: Cow::Borrowed("summarize"),
                title: Some("Summarize Memories".to_string()),
                description: Some(Cow::Borrowed(
                    "Turn episodic logs within a time window into a concise, reusable summary with provenance.",
                )),
                input_schema: summarize_schema.clone(),
                output_schema: None,
                annotations: Some(
                    ToolAnnotations::with_title("Summarize Memories")
                        .destructive(false)
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
                "Enumerate distinct tags for a project: replace {project_id} and call readResource"
                    .into(),
            ),
            mime_type: Some(super::format::APPLICATION_JSON.into()),
        };

        vec![tags_template.no_annotation()]
    }
}

fn resource_memory_types(
    _server: &RustyMemMcpServer,
    _request: ReadResourceRequestParam,
) -> registry::ResourceFuture {
    Box::pin(async move {
        Ok(ReadResourceResult {
            contents: vec![json_resource_contents(
                MEMORY_TYPES_URI,
                memory_types_payload(),
            )],
        })
    })
}

fn resource_health(
    server: &RustyMemMcpServer,
    _request: ReadResourceRequestParam,
) -> registry::ResourceFuture {
    let processing = server.processing.clone();
    Box::pin(async move {
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
    })
}

fn resource_projects(
    server: &RustyMemMcpServer,
    _request: ReadResourceRequestParam,
) -> registry::ResourceFuture {
    let processing = server.processing.clone();
    Box::pin(async move {
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
    })
}

fn resource_settings(
    _server: &RustyMemMcpServer,
    _request: ReadResourceRequestParam,
) -> registry::ResourceFuture {
    Box::pin(async move {
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
    })
}

fn resource_usage(
    _server: &RustyMemMcpServer,
    _request: ReadResourceRequestParam,
) -> registry::ResourceFuture {
    Box::pin(async move {
        let usage = serde_json::json!({
            "title": "Rusty Memory MCP Usage",
            "policy": [
                "Do not paste or concatenate large documents in prompts.",
                "Index text with `push` first; use `search` to retrieve.",
                "Prefer filters: project_id, memory_type, tags, time_range.",
                "Keep query_text concise (<= 512 chars).",
                "Use summarize to consolidate episodic → semantic summaries.",
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
                    "name": "Summarize",
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
    })
}

fn tool_push(server: &RustyMemMcpServer, request: CallToolRequestParam) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_push(&processing, request.arguments).await })
}

fn tool_search(server: &RustyMemMcpServer, request: CallToolRequestParam) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_search(&processing, request.arguments).await })
}

fn tool_get_collections(
    server: &RustyMemMcpServer,
    _request: CallToolRequestParam,
) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_list_collections(&processing).await })
}

fn tool_new_collection(
    server: &RustyMemMcpServer,
    request: CallToolRequestParam,
) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_create_collection(&processing, request.arguments).await })
}

fn tool_metrics(
    server: &RustyMemMcpServer,
    _request: CallToolRequestParam,
) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_metrics(&processing).await })
}

fn tool_summarize(
    server: &RustyMemMcpServer,
    request: CallToolRequestParam,
) -> registry::ToolFuture {
    let processing = server.processing.clone();
    Box::pin(async move { handle_summarize(&processing, request.arguments).await })
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
                "Use this server to index, search, and summarize project memories for agents. Index source text, then retrieve concise context via semantic search with project/type/tag/time filters; summarize time‑bounded entries when needed.".into(),
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
            let uri = request.uri.clone();
            if uri.starts_with(PROJECT_TAGS_PREFIX) && uri.ends_with(PROJECT_TAGS_SUFFIX) {
                let project_segment =
                    &uri[PROJECT_TAGS_PREFIX.len()..uri.len() - PROJECT_TAGS_SUFFIX.len()];
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
                return Ok(ReadResourceResult {
                    contents: vec![json_resource_contents(&uri, serialize_json(&payload, &uri))],
                });
            }

            if let Some(handler) = self.registry.resources.get(uri.as_str()) {
                return handler(self, request).await;
            }

            Err(McpError::invalid_params(
                format!("Unknown resource URI: {uri}"),
                None,
            ))
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            if let Some(handler) = self.registry.tools.get(request.name.as_ref()) {
                return handler(self, request).await;
            }

            Err(McpError::invalid_params(
                format!("Unknown tool: {}", request.name),
                None,
            ))
        }
    }
}
