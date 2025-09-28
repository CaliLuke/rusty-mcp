use crate::config::get_config;
use crate::embedding::{EmbeddingClient, get_embedding_client};
use crate::metrics::{CodeMetrics, MetricsSnapshot};
use crate::qdrant::{QdrantError, QdrantService};
use semchunk_rs::Chunker;
use std::sync::Arc;
use thiserror::Error;

/// Errors emitted by the document processing pipeline.
#[derive(Debug, Error)]
pub enum ProcessingError {
    /// Embedding provider failed to produce vectors for the input text.
    #[error("Failed to generate embeddings: {0}")]
    Embedding(#[from] crate::embedding::EmbeddingClientError),
    /// Qdrant rejected an indexing operation.
    #[error("Failed to index document: {0}")]
    Indexing(#[from] QdrantError),
}

/// High-level orchestration of chunking, embedding, and indexing.
pub struct ProcessingService {
    embedding_client: Box<dyn EmbeddingClient + Send + Sync>,
    qdrant_service: QdrantService,
    metrics: Arc<CodeMetrics>,
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
        tracing::debug!(collection = %config.qdrant_collection_name, vector_size, "Ensuring primary collection");
        qdrant_service
            .create_collection_if_not_exists(&config.qdrant_collection_name, vector_size)
            .await
            .expect("Failed to ensure Qdrant collection exists");
        tracing::debug!(collection = %config.qdrant_collection_name, "Primary collection ready");

        Self {
            embedding_client,
            qdrant_service,
            metrics: Arc::new(CodeMetrics::new()),
        }
    }

    /// Chunk, embed, and index a document, returning the produced chunk count.
    pub async fn process_and_index(
        &self,
        collection_name: &str,
        text: String,
    ) -> Result<usize, ProcessingError> {
        tracing::info!(collection = collection_name, "Processing document");
        let config = get_config();
        self.ensure_collection(collection_name).await?;
        let chunks = chunk_text(&text, config.text_splitter_chunk_size);
        let chunk_count = chunks.len();
        let embeddings = self
            .embedding_client
            .generate_embeddings(chunks.clone())
            .await?;

        self.qdrant_service
            .index_points(collection_name, chunks, embeddings)
            .await?;

        self.metrics.record_document(chunk_count as u64);
        tracing::info!(
            collection = collection_name,
            chunks = chunk_count,
            "Document indexed"
        );

        Ok(chunk_count)
    }

    /// Ensure that the target collection exists within Qdrant.
    pub async fn ensure_collection(&self, collection_name: &str) -> Result<(), ProcessingError> {
        let config = get_config();
        let vector_size = config.embedding_dimension as u64;
        self.qdrant_service
            .create_collection_if_not_exists(collection_name, vector_size)
            .await
            .map_err(ProcessingError::from)
            .map(|()| {
                tracing::debug!(collection = collection_name, "Collection ensured");
            })
    }

    /// Create a new collection (or upsert an existing one) with the desired vector size.
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
            .map_err(ProcessingError::from)
            .map(|()| {
                tracing::info!(
                    collection = collection_name,
                    vector_size = size,
                    "Collection created"
                );
            })
    }

    /// Enumerate all collections currently known to Qdrant.
    pub async fn list_collections(&self) -> Result<Vec<String>, ProcessingError> {
        self.qdrant_service
            .list_collections()
            .await
            .map_err(ProcessingError::from)
    }

    /// Return the current ingestion metrics snapshot.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }
}

fn chunk_text(text: &str, chunk_size: usize) -> Vec<String> {
    let chunker = Chunker::new(
        chunk_size,
        Box::new(|segment: &str| {
            let tokens = segment.split_whitespace().count();
            if tokens == 0 && !segment.is_empty() {
                1
            } else {
                tokens
            }
        }),
    );

    chunker.chunk(text)
}

#[cfg(test)]
mod tests {
    use super::chunk_text;

    #[test]
    fn chunk_text_respects_chunk_size() {
        let text = "one two three four five";
        let chunks = chunk_text(text, 2);
        assert_eq!(chunks, vec!["one two", "three four", "five"]);
    }

    #[test]
    fn chunk_text_handles_empty_input() {
        let chunks = chunk_text("", 4);
        assert!(chunks.is_empty());
    }
}
