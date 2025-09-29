//! Processing service coordinating chunking, embedding, and Qdrant operations.

use crate::{
    config::get_config,
    embedding::{EmbeddingClient, get_embedding_client},
    metrics::{CodeMetrics, MetricsSnapshot},
    processing::{
        chunking::{chunk_text, determine_chunk_size},
        mappers::{dedupe_chunks, map_scored_point},
        sanitize::{sanitize_memory_type, sanitize_project_id, sanitize_tags},
        types::{
            IngestMetadata, ProcessingError, ProcessingOutcome, QdrantHealthSnapshot, SearchError,
            SearchHit, SearchRequest,
        },
    },
    qdrant::{self, IndexSummary, PointInsert, QdrantService},
};
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Coordinates the full ingestion pipeline: semantic chunking, embedding, and Qdrant writes.
///
/// The service owns long-lived handles to the embedding client, Qdrant transport, and metrics
/// registry so that both the HTTP surface and the MCP tools reuse the same components.
/// Construct the service once near process start and share it through an `Arc`.
pub struct ProcessingService {
    embedding_client: Box<dyn EmbeddingClient + Send + Sync>,
    qdrant_service: QdrantService,
    metrics: Arc<CodeMetrics>,
}

/// Abstraction over the processing pipeline used by external surfaces (HTTP, MCP).
#[async_trait]
pub trait ProcessingApi: Send + Sync {
    /// Chunk, embed, and index raw text into the target collection.
    async fn process_and_index(
        &self,
        collection_name: &str,
        text: String,
        metadata: IngestMetadata,
    ) -> Result<ProcessingOutcome, ProcessingError>;

    /// Create or update a collection with the desired vector size.
    async fn create_collection(
        &self,
        collection_name: &str,
        vector_size: Option<u64>,
    ) -> Result<(), ProcessingError>;

    /// Enumerate collections managed by the storage backend.
    async fn list_collections(&self) -> Result<Vec<String>, ProcessingError>;

    /// Retrieve the current metrics snapshot for diagnostics.
    fn metrics_snapshot(&self) -> MetricsSnapshot;
}

impl ProcessingService {
    /// Build a new processing service, initializing backing services as needed.
    pub async fn new() -> Self {
        let config = get_config();
        tracing::info!("Initializing embedding client");
        let embedding_client = get_embedding_client();
        tracing::info!("Embedding client initialized");
        let qdrant_service = QdrantService::new().expect("Failed to connect to Qdrant");
        let vector_size = config.embedding_dimension as u64;
        tracing::debug!(
            collection = %config.qdrant_collection_name,
            vector_size,
            "Ensuring primary collection"
        );
        qdrant_service
            .create_collection_if_not_exists(&config.qdrant_collection_name, vector_size)
            .await
            .expect("Failed to ensure Qdrant collection exists");
        qdrant_service
            .ensure_payload_indexes(&config.qdrant_collection_name)
            .await
            .expect("Failed to ensure Qdrant payload indexes");
        tracing::debug!(collection = %config.qdrant_collection_name, "Primary collection ready");

        Self {
            embedding_client,
            qdrant_service,
            metrics: Arc::new(CodeMetrics::new()),
        }
    }

    /// Chunk, embed, and index a document.
    pub async fn process_and_index(
        &self,
        collection_name: &str,
        text: String,
        metadata: IngestMetadata,
    ) -> Result<ProcessingOutcome, ProcessingError> {
        tracing::info!(collection = collection_name, "Processing document");
        let config = get_config();
        self.ensure_collection(collection_name).await?;
        let chunk_size = determine_chunk_size(
            config.text_splitter_chunk_size,
            config.embedding_provider,
            &config.embedding_model,
            config.text_splitter_use_safe_defaults,
        );
        let overlap = config.text_splitter_chunk_overlap.unwrap_or(0);
        tracing::debug!(
            chunk_size,
            override = config.text_splitter_chunk_size,
            provider = ?config.embedding_provider,
            model = %config.embedding_model,
            overlap,
            use_safe_defaults = config.text_splitter_use_safe_defaults,
            "Derived chunk size"
        );
        let chunks = chunk_text(
            &text,
            chunk_size,
            overlap,
            config.embedding_provider,
            &config.embedding_model,
        )?;
        let (prepared_chunks, skipped_duplicates) = dedupe_chunks(chunks);
        let texts: Vec<String> = prepared_chunks
            .iter()
            .map(|chunk| chunk.text.clone())
            .collect();
        let embeddings = if texts.is_empty() {
            Vec::new()
        } else {
            self.embedding_client.generate_embeddings(texts).await?
        };

        debug_assert_eq!(prepared_chunks.len(), embeddings.len());

        let points: Vec<PointInsert> = prepared_chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, vector)| PointInsert {
                text: chunk.text,
                chunk_hash: chunk.chunk_hash,
                vector,
            })
            .collect();

        let overrides = metadata.into_overrides();
        let IndexSummary { inserted, updated } = self
            .qdrant_service
            .index_points(collection_name, points, &overrides)
            .await?;

        let chunk_count = inserted + updated;

        self.metrics
            .record_document(chunk_count as u64, chunk_size as u64);
        tracing::info!(
            collection = collection_name,
            chunks = chunk_count,
            chunk_size,
            inserted,
            updated,
            skipped_duplicates,
            "Document indexed"
        );

        Ok(ProcessingOutcome {
            chunk_count,
            chunk_size,
            inserted,
            updated,
            skipped_duplicates,
        })
    }

    /// Execute a semantic search query against Qdrant using the configured embedding provider.
    pub async fn search_memories(
        &self,
        request: SearchRequest,
    ) -> Result<Vec<SearchHit>, SearchError> {
        let config = get_config();
        let SearchRequest {
            query_text,
            collection,
            project_id,
            memory_type,
            tags,
            time_range,
            limit,
            score_threshold,
        } = request;

        let collection_name = collection.unwrap_or_else(|| config.qdrant_collection_name.clone());
        let mut vectors = self
            .embedding_client
            .generate_embeddings(vec![query_text])
            .await?;
        let vector = vectors.pop().ok_or(SearchError::EmptyEmbedding)?;

        let expected = config.embedding_dimension;
        let actual = vector.len();
        if actual != expected {
            return Err(SearchError::DimensionMismatch { expected, actual });
        }

        let default_limit = config.search_default_limit;
        let max_limit = config.search_max_limit;
        let default_threshold = config.search_default_score_threshold;

        let limit = limit.unwrap_or(default_limit).clamp(1, max_limit);
        let threshold = score_threshold.unwrap_or(default_threshold).clamp(0.0, 1.0);

        let filter_args = qdrant::SearchFilterArgs {
            project_id: sanitize_project_id(project_id),
            memory_type: sanitize_memory_type(memory_type),
            tags: sanitize_tags(tags),
            time_range: time_range.map(|range| qdrant::SearchTimeRange {
                start: range.start,
                end: range.end,
            }),
        };

        let filter = qdrant::build_search_filter(&filter_args);

        let hits = self
            .qdrant_service
            .search_points(
                &collection_name,
                vector,
                filter,
                limit,
                Some(threshold),
                None,
            )
            .await?;

        Ok(hits.into_iter().map(map_scored_point).collect())
    }

    /// Ensure that the target collection exists within Qdrant.
    pub async fn ensure_collection(&self, collection_name: &str) -> Result<(), ProcessingError> {
        let config = get_config();
        let vector_size = config.embedding_dimension as u64;
        self.qdrant_service
            .create_collection_if_not_exists(collection_name, vector_size)
            .await
            .map_err(ProcessingError::from)?;
        self.qdrant_service
            .ensure_payload_indexes(collection_name)
            .await
            .map_err(ProcessingError::from)?;
        tracing::debug!(collection = collection_name, "Collection ensured");
        Ok(())
    }

    /// Create or resize a collection with the desired vector size.
    pub async fn create_collection(
        &self,
        collection_name: &str,
        vector_size: Option<u64>,
    ) -> Result<(), ProcessingError> {
        let size = vector_size.unwrap_or_else(|| {
            let config = get_config();
            config.embedding_dimension as u64
        });

        self.qdrant_service
            .create_collection(collection_name, size)
            .await
            .map_err(ProcessingError::from)?;
        self.qdrant_service
            .ensure_payload_indexes(collection_name)
            .await
            .map_err(ProcessingError::from)?;
        tracing::info!(
            collection = collection_name,
            vector_size = size,
            "Collection created"
        );
        Ok(())
    }

    /// Enumerate all collections currently known to Qdrant.
    pub async fn list_collections(&self) -> Result<Vec<String>, ProcessingError> {
        self.qdrant_service
            .list_collections()
            .await
            .map_err(ProcessingError::from)
    }

    /// Enumerate distinct project identifiers observed in the target collection.
    pub async fn list_projects(
        &self,
        collection_name: &str,
    ) -> Result<BTreeSet<String>, ProcessingError> {
        self.qdrant_service
            .list_projects(collection_name)
            .await
            .map_err(ProcessingError::from)
    }

    /// Enumerate distinct tags observed in the target collection, optionally scoped by project.
    pub async fn list_tags(
        &self,
        collection_name: &str,
        project_id: Option<&str>,
    ) -> Result<BTreeSet<String>, ProcessingError> {
        self.qdrant_service
            .list_tags(collection_name, project_id)
            .await
            .map_err(ProcessingError::from)
    }

    /// Return the current ingestion metrics snapshot.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Probe Qdrant to surface a lightweight health snapshot for MCP resources.
    pub async fn qdrant_health(&self) -> QdrantHealthSnapshot {
        let config = get_config();
        match self.qdrant_service.list_collections().await {
            Ok(collections) => {
                let default_present = collections
                    .iter()
                    .any(|name| name == &config.qdrant_collection_name);
                QdrantHealthSnapshot {
                    reachable: true,
                    default_collection_present: default_present,
                    error: None,
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "Qdrant health probe failed");
                QdrantHealthSnapshot {
                    reachable: false,
                    default_collection_present: false,
                    error: Some(error.to_string()),
                }
            }
        }
    }
}

#[async_trait]
impl ProcessingApi for ProcessingService {
    async fn process_and_index(
        &self,
        collection_name: &str,
        text: String,
        metadata: IngestMetadata,
    ) -> Result<ProcessingOutcome, ProcessingError> {
        ProcessingService::process_and_index(self, collection_name, text, metadata).await
    }

    async fn create_collection(
        &self,
        collection_name: &str,
        vector_size: Option<u64>,
    ) -> Result<(), ProcessingError> {
        ProcessingService::create_collection(self, collection_name, vector_size).await
    }

    async fn list_collections(&self) -> Result<Vec<String>, ProcessingError> {
        ProcessingService::list_collections(self).await
    }

    fn metrics_snapshot(&self) -> MetricsSnapshot {
        ProcessingService::metrics_snapshot(self)
    }
}
