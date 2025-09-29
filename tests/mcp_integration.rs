use std::sync::Arc;

use httpmock::{Method::GET, Method::POST, Method::PUT, Mock, MockServer};
use regex::Regex;
use rmcp::{
    handler::client::ClientHandler,
    model::{self, CallToolRequestParam, ClientInfo, PaginatedRequestParam},
    service::{RoleClient, RoleServer, RunningService, Service, serve_directly},
    transport::async_rw::AsyncRwTransport,
};
use rustymcp::{config, logging, mcp::RustyMemMcpServer, processing::ProcessingService};
use serde_json::json;
use tokio::{io::split, sync::OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static MOCK_SERVER: OnceCell<&'static MockServer> = OnceCell::const_new();
static MOCK_HANDLES: OnceCell<Vec<Mock<'static>>> = OnceCell::const_new();

macro_rules! test_log {
    ($($arg:tt)*) => {
        if std::env::var("RUSTY_MEM_TEST_LOG").is_ok() {
            eprintln!($($arg)*);
        }
    };
}

fn set_env(key: &str, value: &str) {
    // SAFETY: Tests run in a single process and establish deterministic configuration upfront.
    unsafe { std::env::set_var(key, value) }
}

#[derive(Clone, Default)]
struct DummyClientHandler;

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

struct TestHarness {
    service: RunningService<RoleClient, DummyClientHandler>,
    server: RunningService<RoleServer, RustyMemMcpServer>,
}

impl TestHarness {
    async fn new() -> Self {
        test_log!("[harness] init start");
        INIT.get_or_init(|| async {
            test_log!("[harness:init] starting mock server");
            let mock_server_owned = MockServer::start_async().await;
            let mock_server = Box::leak(Box::new(mock_server_owned));
            let base_url = mock_server.base_url();

            test_log!("[harness:init] configuring environment");
            set_env("QDRANT_URL", &base_url);
            set_env("QDRANT_COLLECTION_NAME", "rusty-mem");
            set_env("EMBEDDING_PROVIDER", "openai");
            set_env("EMBEDDING_MODEL", "nomic-embed-text:latest");
            set_env("EMBEDDING_DIMENSION", "768");
            set_env("TEXT_SPLITTER_CHUNK_SIZE", "4");
            set_env("OLLAMA_URL", "http://127.0.0.1:11434");
            set_env("RUST_LOG", "warn");

            MOCK_SERVER.set(mock_server).ok();

            let server = MOCK_SERVER.get().expect("mock server initialized");
            let collections_regex = Regex::new(r"^/collections/").unwrap();

            test_log!("[harness:init] registering http mocks");
            let mocks: Vec<Mock<'static>> = vec![
                server
                    .mock_async(|when, then| {
                        when.method(GET).path("/collections");
                        then.status(200).json_body(json!({
                            "status": "ok",
                            "time": 0.0,
                            "result": {
                                "collections": [{ "name": "rusty-mem" }]
                            }
                        }));
                    })
                    .await,
                server
                    .mock_async({
                        let collections_regex = collections_regex.clone();
                        move |when, then| {
                            when.method(GET).path_matches(collections_regex.clone());
                            then.status(200).json_body(json!({
                                "status": "ok",
                                "time": 0.0,
                                "result": {}
                            }));
                        }
                    })
                    .await,
                server
                    .mock_async({
                        let collections_regex = collections_regex.clone();
                        move |when, then| {
                            when.method(POST)
                                .path_matches(collections_regex.clone())
                                .path_contains("/points/query");
                            then.status(200).json_body(json!({
                                "status": "ok",
                                "time": 0.0,
                                "result": [
                                    {
                                        "id": "memory-1",
                                        "score": 0.9,
                                        "payload": {
                                            "text": "Example memory",
                                            "project_id": "default",
                                            "memory_type": "semantic",
                                            "timestamp": "2025-01-01T00:00:00Z",
                                            "tags": ["alpha"]
                                        }
                                    }
                                ]
                            }));
                        }
                    })
                    .await,
                server
                    .mock_async({
                        let collections_regex = collections_regex.clone();
                        move |when, then| {
                            when.method(PUT)
                                .path_matches(collections_regex.clone())
                                .path_contains("/points");
                            then.status(200).json_body(json!({
                                "status": "ok",
                                "time": 0.0,
                                "result": {
                                    "operation_id": 1,
                                    "status": "completed"
                                }
                            }));
                        }
                    })
                    .await,
                server
                    .mock_async({
                        let collections_regex = collections_regex.clone();
                        move |when, then| {
                            when.method(PUT).path_matches(collections_regex.clone());
                            then.status(200).json_body(json!({
                                "status": "ok",
                                "time": 0.0,
                                "result": {}
                            }));
                        }
                    })
                    .await,
            ];

            MOCK_HANDLES.set(mocks).ok();

            test_log!("[harness:init] initializing config & logging");
            config::init_config();
            logging::init_tracing();
            test_log!("[harness:init] ready");
        })
        .await;

        test_log!("[harness] building processing service");
        let processing = Arc::new(ProcessingService::new().await);
        test_log!("[harness] processing ready");
        let server = RustyMemMcpServer::new(processing);

        let (client_stream, server_stream) = tokio::io::duplex(16 * 1024);
        let (client_read, client_write) = split(client_stream);
        let (server_read, server_write) = split(server_stream);

        let client_transport = AsyncRwTransport::new_client(client_read, client_write);
        let server_transport = AsyncRwTransport::new_server(server_read, server_write);

        let server_info = server.get_info();
        let client_handler = DummyClientHandler;
        let client_info = ClientHandler::get_info(&client_handler);

        let server =
            serve_directly::<RoleServer, _, _, _, _>(server, server_transport, Some(client_info));

        test_log!("[harness] starting client service");
        let service = serve_directly::<RoleClient, _, _, _, _>(
            client_handler,
            client_transport,
            Some(server_info),
        );
        test_log!("[harness] client service ready");

        Self { service, server }
    }

    async fn shutdown(self) {
        test_log!("[harness] shutdown start");
        let Self { service, server } = self;
        let _ = service.cancel().await;
        let _ = server.cancel().await;
        test_log!("[harness] shutdown complete");
    }
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let harness = TestHarness::new().await;
    let service = &harness.service;

    let info = service
        .peer_info()
        .expect("server info should be initialized");
    assert_eq!(info.server_info.name, "rusty-mem");
    assert!(info.capabilities.tools.is_some());

    let tools_result = service
        .list_tools(Some(PaginatedRequestParam { cursor: None }))
        .await
        .expect("list_tools");

    let names: Vec<_> = tools_result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect();

    assert!(names.contains(&"push"));
    assert!(names.contains(&"search"));
    assert!(names.contains(&"get-collections"));
    assert!(names.contains(&"new-collection"));
    assert!(names.contains(&"metrics"));

    harness.shutdown().await;
}

#[tokio::test]
async fn index_tool_invokes_processing() {
    let harness = TestHarness::new().await;
    let service = &harness.service;

    let response = service
        .call_tool(CallToolRequestParam {
            name: "push".into(),
            arguments: Some(
                json!({
                    "text": "Hello world",
                    "collection": "mcp-test"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await
        .expect("index tool call");

    assert_eq!(response.is_error, Some(false));
    let payload = response.structured_content.expect("structured payload");
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["collection"], "mcp-test");
    assert!(payload["chunksIndexed"].as_u64().is_some());
    assert!(payload["chunkSize"].as_u64().is_some());

    let metrics_response = service
        .call_tool(CallToolRequestParam {
            name: "metrics".into(),
            arguments: Some(json!({}).as_object().unwrap().clone()),
        })
        .await
        .expect("metrics tool call");
    assert_eq!(metrics_response.is_error, Some(false));
    let metrics_payload = metrics_response
        .structured_content
        .expect("structured metrics payload");
    assert!(metrics_payload["lastChunkSize"].as_u64().is_some());

    harness.shutdown().await;
}

#[tokio::test]
async fn search_tool_returns_results() {
    let harness = TestHarness::new().await;
    let service = &harness.service;

    let response = service
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                json!({
                    "query_text": "architecture notes",
                    "tags": ["alpha"],
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        })
        .await
        .expect("search tool call");

    assert_eq!(response.is_error, Some(false));
    let payload = response
        .structured_content
        .expect("structured search payload");
    assert_eq!(payload["collection"], "rusty-mem");
    assert_eq!(payload["limit"], json!(5));
    assert_eq!(payload["scoreThreshold"], json!(0.25));
    let results = payload["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "memory-1");
    assert_eq!(results[0]["project_id"], "default");

    harness.shutdown().await;
}

#[tokio::test]
async fn invalid_payload_returns_error() {
    let harness = TestHarness::new().await;
    let service = &harness.service;

    let err = service
        .call_tool(CallToolRequestParam {
            name: "push".into(),
            arguments: Some(json!({ "text": "" }).as_object().unwrap().clone()),
        })
        .await
        .expect_err("index should fail");

    match err {
        rmcp::service::ServiceError::McpError(data) => {
            assert_eq!(data.code, model::ErrorCode::INVALID_PARAMS);
        }
        other => panic!("expected MCP error, got {other:?}"),
    }

    harness.shutdown().await;
}
