//! HTTP surface for Rusty Memory.
//!
//! This module exposes a compact Axum router with a handful of endpoints:
//!
//! - `POST /index` – Chunk a raw document, generate embeddings, and persist them in Qdrant.
//!   Accepts optional metadata (`collection`, `project_id`, `memory_type`, `tags`, `source_uri`) and
//!   returns indexing counters (`chunks_indexed`, `chunk_size`, `inserted`, `updated`, `skipped_duplicates`).
//! - `GET /collections` – List Qdrant collections managed by this server.
//! - `POST /collections` – Create or resize a collection (idempotent).
//! - `GET /metrics` – Observe ingestion counters and the last chunk size used.
//! - `GET /commands` – Machine-readable command catalog for quick discovery by tools/hosts.
//!
//! The HTTP surface shares the same processing pipeline with the MCP server, so behavior is
//! identical across interfaces.

use crate::config::get_config;
use crate::processing::{IngestMetadata, ProcessingApi, ProcessingError};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Build the HTTP router exposing the ingestion API surface.
pub fn create_router<S>(service: Arc<S>) -> Router
where
    S: ProcessingApi + 'static,
{
    Router::new()
        .route("/index", post(index_document::<S>))
        .route(
            "/collections",
            get(list_collections::<S>).post(create_collection::<S>),
        )
        .route("/metrics", get(get_metrics::<S>))
        .route("/commands", get(get_commands))
        .with_state(service)
}

/// Request body for the `POST /index` endpoint.
#[derive(Deserialize)]
struct IndexRequest {
    /// Raw document contents to chunk and index.
    text: String,
    /// Optional collection override (defaults to `QDRANT_COLLECTION_NAME`).
    #[serde(default)]
    collection: Option<String>,
    /// Optional project identifier persisted with each chunk (defaults to `"default"`).
    #[serde(default)]
    project_id: Option<String>,
    /// Optional memory classification (`episodic` | `semantic` | `procedural`).
    #[serde(default)]
    memory_type: Option<String>,
    /// Optional tag list applied to each chunk.
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// Optional source URI (file path or URL) for traceability.
    #[serde(default)]
    source_uri: Option<String>,
}

/// Success response for the `POST /index` endpoint.
#[derive(Serialize)]
struct IndexResponse {
    /// Number of chunks produced for the provided document.
    chunks_indexed: usize,
    /// Effective chunk size used for this ingestion.
    chunk_size: usize,
    /// Number of new vectors inserted into the collection.
    inserted: usize,
    /// Number of existing vectors updated in place (typically 0 with UUID ids).
    updated: usize,
    /// Number of duplicate chunks skipped within this request.
    skipped_duplicates: usize,
}

/// Index a document into the target collection.
///
/// This handler accepts raw text and optional metadata, derives a chunk size (unless
/// `TEXT_SPLITTER_CHUNK_SIZE` is set), performs semantic chunking and embedding, and upserts
/// the resulting vectors to Qdrant.
async fn index_document<S>(
    State(service): State<Arc<S>>,
    Json(request): Json<IndexRequest>,
) -> Result<Json<IndexResponse>, AppError>
where
    S: ProcessingApi,
{
    let IndexRequest {
        text,
        collection,
        project_id,
        memory_type,
        tags,
        source_uri,
    } = request;
    let collection_name = collection.unwrap_or_else(|| get_config().qdrant_collection_name.clone());
    let metadata = IngestMetadata {
        project_id,
        memory_type,
        tags,
        source_uri,
    };
    let outcome = service
        .process_and_index(&collection_name, text, metadata)
        .await?;
    tracing::info!(
        collection = collection_name,
        chunks = outcome.chunk_count,
        chunk_size = outcome.chunk_size,
        inserted = outcome.inserted,
        updated = outcome.updated,
        skipped_duplicates = outcome.skipped_duplicates,
        "Index request completed"
    );
    Ok(Json(IndexResponse {
        chunks_indexed: outcome.chunk_count,
        chunk_size: outcome.chunk_size,
        inserted: outcome.inserted,
        updated: outcome.updated,
        skipped_duplicates: outcome.skipped_duplicates,
    }))
}

/// Response body for `GET /collections`.
#[derive(Serialize)]
struct CollectionsResponse {
    collections: Vec<String>,
}

/// List Qdrant collections available to this server.
async fn list_collections<S>(
    State(service): State<Arc<S>>,
) -> Result<Json<CollectionsResponse>, AppError>
where
    S: ProcessingApi,
{
    let collections = service.list_collections().await?;
    Ok(Json(CollectionsResponse { collections }))
}

/// Request body for `POST /collections` to create/resize a collection.
#[derive(Deserialize)]
struct CreateCollectionRequest {
    /// Name of the collection to create or resize.
    name: String,
    /// Optional vector size override (defaults to `EMBEDDING_DIMENSION`).
    #[serde(default)]
    vector_size: Option<u64>,
}

/// Create or resize a collection.
async fn create_collection<S>(
    State(service): State<Arc<S>>,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<(), AppError>
where
    S: ProcessingApi,
{
    service
        .create_collection(&request.name, request.vector_size)
        .await?;
    Ok(())
}

/// Return a concise metrics snapshot with document/chunk counters and the last chunk size.
async fn get_metrics<S>(State(service): State<Arc<S>>) -> Result<Json<MetricsResponse>, AppError>
where
    S: ProcessingApi,
{
    let snapshot = service.metrics_snapshot();
    Ok(Json(MetricsResponse {
        documents_indexed: snapshot.documents_indexed,
        chunks_indexed: snapshot.chunks_indexed,
        last_chunk_size: snapshot.last_chunk_size,
    }))
}

/// Response body for `GET /metrics`.
#[derive(Serialize)]
struct MetricsResponse {
    documents_indexed: u64,
    chunks_indexed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_chunk_size: Option<u64>,
}

/// Descriptor for a single command in the discovery catalog.
#[derive(Serialize)]
struct CommandDescriptor {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_example: Option<serde_json::Value>,
}

/// Response body for `GET /commands`.
#[derive(Serialize)]
struct CommandsResponse {
    commands: Vec<CommandDescriptor>,
}

/// Enumerate supported HTTP commands for discovery/UX in hosts and tools.
async fn get_commands() -> Json<CommandsResponse> {
    Json(CommandsResponse {
        commands: vec![
            CommandDescriptor {
                name: "index",
                method: "POST",
                path: "/index",
                description: "Chunk a raw document, generate embeddings, and persist them in Qdrant. Response returns { \"chunks_indexed\": number, \"chunk_size\": number }.",
                request_example: Some(json!({
                    "text": "Document contents",
                    "collection": "optional-collection",
                    "project_id": "project-123",
                    "memory_type": "episodic",
                    "tags": ["alpha", "beta"],
                    "source_uri": "https://example.org/origin"
                })),
            },
            CommandDescriptor {
                name: "list_collections",
                method: "GET",
                path: "/collections",
                description: "Return the names of Qdrant collections managed by this server.",
                request_example: None,
            },
            CommandDescriptor {
                name: "create_collection",
                method: "POST",
                path: "/collections",
                description: "Create a new Qdrant collection (non-destructive if it already exists).",
                request_example: Some(json!({
                    "name": "my-collection",
                    "vector_size": 1536
                })),
            },
            CommandDescriptor {
                name: "metrics",
                method: "GET",
                path: "/metrics",
                description: "Return ingestion counters useful for observability dashboards.",
                request_example: None,
            },
        ],
    })
}

struct AppError(ProcessingError);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl From<ProcessingError> for AppError {
    fn from(inner: ProcessingError) -> Self {
        Self(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::{create_router, get_commands};
    use crate::config::{CONFIG, Config, EmbeddingProvider};
    use crate::metrics::MetricsSnapshot;
    use crate::processing::{IngestMetadata, ProcessingApi, ProcessingOutcome};
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
    };
    use serde_json::json;
    use std::sync::{Arc, Once};
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    #[tokio::test]
    async fn commands_catalog_exposes_index_endpoint() {
        let response = get_commands().await;
        let commands = response.0.commands;
        let index = commands
            .iter()
            .find(|cmd| cmd.name == "index")
            .expect("index command present");

        assert_eq!(index.method, "POST");
        assert_eq!(index.path, "/index");
        assert!(index.description.to_lowercase().contains("chunk"));

        // ensure catalog exposes multiple commands for host discovery
        assert!(commands.len() >= 3);
    }

    #[tokio::test]
    async fn index_route_accepts_metadata_payload() {
        ensure_test_config();
        let outcome = ProcessingOutcome {
            chunk_count: 2,
            chunk_size: 512,
            inserted: 2,
            updated: 0,
            skipped_duplicates: 0,
        };
        let service = Arc::new(StubProcessingService::new(outcome));
        let app = create_router(service.clone());

        let payload = json!({
            "text": "Document body",
            "collection": "custom-collection",
            "project_id": "proj-42",
            "memory_type": "semantic",
            "tags": ["alpha", "beta"],
            "source_uri": "https://example.org/doc"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/index")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .expect("request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["chunks_indexed"], 2);
        assert_eq!(json["chunk_size"], 512);

        let calls = service.recorded_calls().await;
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.collection, "custom-collection");
        assert_eq!(call.text, "Document body");
        assert_eq!(call.metadata.project_id.as_deref(), Some("proj-42"));
        assert_eq!(call.metadata.memory_type.as_deref(), Some("semantic"));
        assert_eq!(
            call.metadata.tags.as_ref(),
            Some(&vec!["alpha".into(), "beta".into()])
        );
        assert_eq!(
            call.metadata.source_uri.as_deref(),
            Some("https://example.org/doc")
        );
    }

    #[derive(Clone, Debug)]
    struct IngestCall {
        collection: String,
        text: String,
        metadata: IngestMetadata,
    }

    #[derive(Clone)]
    struct StubProcessingService {
        calls: Arc<Mutex<Vec<IngestCall>>>,
        outcome: ProcessingOutcome,
    }

    impl StubProcessingService {
        fn new(outcome: ProcessingOutcome) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                outcome,
            }
        }

        async fn recorded_calls(&self) -> Vec<IngestCall> {
            self.calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl ProcessingApi for StubProcessingService {
        async fn process_and_index(
            &self,
            collection_name: &str,
            text: String,
            metadata: IngestMetadata,
        ) -> Result<ProcessingOutcome, crate::processing::ProcessingError> {
            let mut guard = self.calls.lock().await;
            guard.push(IngestCall {
                collection: collection_name.to_string(),
                text,
                metadata,
            });
            Ok(self.outcome)
        }

        async fn create_collection(
            &self,
            _collection_name: &str,
            _vector_size: Option<u64>,
        ) -> Result<(), crate::processing::ProcessingError> {
            Ok(())
        }

        async fn list_collections(
            &self,
        ) -> Result<Vec<String>, crate::processing::ProcessingError> {
            Ok(vec![])
        }

        fn metrics_snapshot(&self) -> MetricsSnapshot {
            MetricsSnapshot {
                documents_indexed: 0,
                chunks_indexed: 0,
                last_chunk_size: None,
            }
        }
    }

    fn ensure_test_config() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = CONFIG.set(Config {
                qdrant_url: "http://127.0.0.1:6333".into(),
                qdrant_collection_name: "default-collection".into(),
                qdrant_api_key: None,
                embedding_provider: EmbeddingProvider::OpenAI,
                text_splitter_chunk_size: None,
                text_splitter_chunk_overlap: None,
                text_splitter_use_safe_defaults: false,
                embedding_model: "test-model".into(),
                embedding_dimension: 256,
                ollama_url: None,
                server_port: None,
                search_default_limit: 5,
                search_max_limit: 50,
                search_default_score_threshold: 0.25,
            });
        });
    }
}
