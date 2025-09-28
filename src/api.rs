use crate::config::get_config;
use crate::processing::{IngestMetadata, ProcessingError, ProcessingService};
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
pub fn create_router(service: Arc<ProcessingService>) -> Router {
    Router::new()
        .route("/index", post(index_document))
        .route(
            "/collections",
            get(list_collections).post(create_collection),
        )
        .route("/metrics", get(get_metrics))
        .route("/commands", get(get_commands))
        .with_state(service)
}

#[derive(Deserialize)]
struct IndexRequest {
    text: String,
    #[serde(default)]
    collection: Option<String>,
}

#[derive(Serialize)]
struct IndexResponse {
    chunks_indexed: usize,
    chunk_size: usize,
    inserted: usize,
    updated: usize,
    skipped_duplicates: usize,
}

async fn index_document(
    State(service): State<Arc<ProcessingService>>,
    Json(request): Json<IndexRequest>,
) -> Result<Json<IndexResponse>, AppError> {
    let collection_name = request
        .collection
        .unwrap_or_else(|| get_config().qdrant_collection_name.clone());
    let outcome = service
        .process_and_index(&collection_name, request.text, IngestMetadata::default())
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

#[derive(Serialize)]
struct CollectionsResponse {
    collections: Vec<String>,
}

async fn list_collections(
    State(service): State<Arc<ProcessingService>>,
) -> Result<Json<CollectionsResponse>, AppError> {
    let collections = service.list_collections().await?;
    Ok(Json(CollectionsResponse { collections }))
}

#[derive(Deserialize)]
struct CreateCollectionRequest {
    name: String,
    #[serde(default)]
    vector_size: Option<u64>,
}

async fn create_collection(
    State(service): State<Arc<ProcessingService>>,
    Json(request): Json<CreateCollectionRequest>,
) -> Result<(), AppError> {
    service
        .create_collection(&request.name, request.vector_size)
        .await?;
    Ok(())
}

async fn get_metrics(
    State(service): State<Arc<ProcessingService>>,
) -> Result<Json<MetricsResponse>, AppError> {
    let snapshot = service.metrics_snapshot();
    Ok(Json(MetricsResponse {
        documents_indexed: snapshot.documents_indexed,
        chunks_indexed: snapshot.chunks_indexed,
        last_chunk_size: snapshot.last_chunk_size,
    }))
}

#[derive(Serialize)]
struct MetricsResponse {
    documents_indexed: u64,
    chunks_indexed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_chunk_size: Option<u64>,
}

#[derive(Serialize)]
struct CommandDescriptor {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_example: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct CommandsResponse {
    commands: Vec<CommandDescriptor>,
}

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
                    "metadata": {
                        "source": "optional identifier"
                    }
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
    use super::get_commands;

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
}
