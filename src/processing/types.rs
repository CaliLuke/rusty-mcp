//! Core data types and error definitions for the processing pipeline.

use crate::{
    config::EmbeddingProvider,
    qdrant::{PayloadOverrides, QdrantError},
};
use anyhow::Error as TokenizerError;
use thiserror::Error;

/// Errors produced while turning raw text into semantic chunks.
#[derive(Debug, Error)]
pub enum ChunkingError {
    /// Ingestion configured an impossible token budget.
    #[error("chunk size must be greater than zero")]
    InvalidChunkSize,
    /// Tokenizer resources were unavailable for the configured model.
    #[error("failed to initialize tokenizer for model '{model}': {source}")]
    Tokenizer {
        /// Embedding model we attempted to load.
        model: String,
        /// Underlying error raised by the tokenizer library.
        #[source]
        source: TokenizerError,
    },
}

/// Errors emitted by the document processing pipeline.
#[derive(Debug, Error)]
pub enum ProcessingError {
    /// Chunking step failed to segment the document.
    #[error("Failed to chunk document: {0}")]
    Chunking(#[from] ChunkingError),
    /// Embedding provider failed to produce vectors for the input text.
    #[error("Failed to generate embeddings: {0}")]
    Embedding(#[from] crate::embedding::EmbeddingClientError),
    /// Qdrant interaction failed during ingestion or metadata queries.
    #[error("Qdrant request failed: {0}")]
    Qdrant(#[from] QdrantError),
}

/// Errors emitted while orchestrating similarity searches.
#[derive(Debug, Error)]
pub enum SearchError {
    /// Embedding provider failed to return vectors for the query text.
    #[error("Failed to generate embeddings: {0}")]
    Embedding(#[from] crate::embedding::EmbeddingClientError),
    /// Qdrant search request returned an error response.
    #[error("Qdrant request failed: {0}")]
    Qdrant(#[from] QdrantError),
    /// Returned embedding dimension does not match configuration.
    #[error("Embedding dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// Expected embedding dimension configured on the server.
        expected: usize,
        /// Actual embedding dimension produced by the provider.
        actual: usize,
    },
    /// Embedding provider returned no vectors.
    #[error("Embedding provider returned no vectors for the query")]
    EmptyEmbedding,
}

/// Summary of a completed ingestion produced by [`crate::processing::ProcessingService::process_and_index`].
#[derive(Debug, Clone, Copy)]
pub struct ProcessingOutcome {
    /// Number of chunks produced for the document.
    pub chunk_count: usize,
    /// Chunk size used during processing.
    pub chunk_size: usize,
    /// Number of new vectors inserted into Qdrant.
    pub inserted: usize,
    /// Number of existing vectors that were updated in place.
    pub updated: usize,
    /// Chunks skipped within the request due to duplicate `chunk_hash`.
    pub skipped_duplicates: usize,
}

/// Reachability and readiness snapshot for Qdrant.
#[derive(Debug, Clone)]
pub struct QdrantHealthSnapshot {
    /// Indicates whether the Qdrant HTTP endpoint responded successfully.
    pub reachable: bool,
    /// Whether the configured default collection is currently present.
    pub default_collection_present: bool,
    /// Optional diagnostic string captured when Qdrant is unreachable.
    pub error: Option<String>,
}

/// Parameters supplied to the search pipeline.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Natural language query text to embed.
    pub query_text: String,
    /// Optional Qdrant collection override.
    pub collection: Option<String>,
    /// Optional payload filter for `project_id`.
    pub project_id: Option<String>,
    /// Optional payload filter for `memory_type`.
    pub memory_type: Option<String>,
    /// Optional contains-any filter for `tags`.
    pub tags: Option<Vec<String>>,
    /// Optional timestamp boundaries for `timestamp` payload field.
    pub time_range: Option<SearchTimeRange>,
    /// Maximum number of results to return (defaults applied downstream).
    pub limit: Option<usize>,
    /// Minimum score accepted from Qdrant (defaults applied downstream).
    pub score_threshold: Option<f32>,
}

/// Inclusive timestamp boundaries expressed as RFC3339 strings.
#[derive(Debug, Clone, Default)]
pub struct SearchTimeRange {
    /// Inclusive start timestamp (`gte`).
    pub start: Option<String>,
    /// Inclusive end timestamp (`lte`).
    pub end: Option<String>,
}

/// Structured search hit returned to API consumers.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Identifier assigned by Qdrant.
    pub id: String,
    /// Similarity score reported by Qdrant.
    pub score: f32,
    /// Stored text payload, if available.
    pub text: Option<String>,
    /// Stored project identifier, if available.
    pub project_id: Option<String>,
    /// Stored memory type, if available.
    pub memory_type: Option<String>,
    /// Stored tags, if available.
    pub tags: Option<Vec<String>>,
    /// Stored timestamp, if available.
    pub timestamp: Option<String>,
    /// Stored source URI, if available.
    pub source_uri: Option<String>,
}

/// Optional metadata passed along with a `push` request.
#[derive(Debug, Default, Clone)]
pub struct IngestMetadata {
    /// Optional project identifier grouped under the memory payload.
    pub project_id: Option<String>,
    /// Optional memory classification (`episodic`/`semantic`/`procedural`).
    pub memory_type: Option<String>,
    /// Optional set of tags to persist for payload filtering.
    pub tags: Option<Vec<String>>,
    /// Optional URI describing the source document for traceability.
    pub source_uri: Option<String>,
}

impl IngestMetadata {
    /// Convert metadata into payload overrides applied during ingestion.
    pub(crate) fn into_overrides(self) -> PayloadOverrides {
        crate::processing::sanitize::to_payload_overrides(self)
    }
}

/// Embedding context-window lookup for external consumers.
pub fn embedding_context_window(provider: EmbeddingProvider, model: &str) -> usize {
    super::chunking::embedding_context_window(provider, model)
}
