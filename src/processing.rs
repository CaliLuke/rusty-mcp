use crate::config::{EmbeddingProvider, get_config};
use crate::embedding::{EmbeddingClient, get_embedding_client};
use crate::metrics::{CodeMetrics, MetricsSnapshot};
use crate::qdrant::{
    self, IndexSummary, PayloadOverrides, PointInsert, QdrantError, QdrantService,
    compute_chunk_hash,
};
use anyhow::Error as TokenizerError;
use semchunk_rs::Chunker;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use thiserror::Error;
use tiktoken_rs::{
    CoreBPE, cl100k_base, get_bpe_from_model, model::get_context_size, o200k_base, p50k_base,
    p50k_edit, r50k_base,
};

type TokenCounter = Box<dyn Fn(&str) -> usize>;

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

/// Summary of a completed ingestion produced by [`ProcessingService::process_and_index`].
///
/// Returning a structured outcome keeps the public API small while still giving callers
/// insight into how the automatic chunk-size heuristics behaved for the most recent request.
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

impl ProcessingService {
    /// Build a new processing service, initializing backing services as needed.
    ///
    /// This eagerly establishes a Qdrant connection, ensures the default collection exists,
    /// and constructs the embedding client referenced by future ingestion calls.
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
        // Ensure standard payload indexes exist for filters.
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
    ///
    /// Text is split using model-aware chunk sizes, embedded via the configured provider, and
    /// flushed to Qdrant in a single batch. Metadata overrides customize the payload without
    /// breaking backwards compatibility. The structured [`ProcessingOutcome`] reports how many
    /// chunks were generated, which chunk size heuristic was applied, and dedupe counters.
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
        );
        tracing::debug!(
            chunk_size,
            override = config.text_splitter_chunk_size,
            provider = ?config.embedding_provider,
            model = %config.embedding_model,
            "Derived chunk size"
        );
        let chunks = chunk_text(
            &text,
            chunk_size,
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
    ///
    /// This helper is idempotent; it only issues a creation call if Qdrant reports the
    /// collection as missing.
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
    ///
    /// When `vector_size` is omitted the service falls back to the embedding dimension from
    /// configuration, keeping Qdrant and the embedding provider in sync.
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
    ///
    /// Callers can expose the snapshot through diagnostics endpoints or dashboards.
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
    fn into_overrides(self) -> PayloadOverrides {
        PayloadOverrides {
            project_id: sanitize_project_id(self.project_id),
            memory_type: sanitize_memory_type(self.memory_type),
            tags: sanitize_tags(self.tags),
            source_uri: sanitize_string(self.source_uri),
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedChunk {
    text: String,
    chunk_hash: String,
}

fn dedupe_chunks(chunks: Vec<String>) -> (Vec<PreparedChunk>, usize) {
    let mut seen = HashSet::new();
    let mut prepared = Vec::new();
    let mut skipped = 0;

    for text in chunks {
        if text.trim().is_empty() {
            continue;
        }
        let hash = compute_chunk_hash(&text);
        if seen.insert(hash.clone()) {
            prepared.push(PreparedChunk {
                text,
                chunk_hash: hash,
            });
        } else {
            skipped += 1;
        }
    }

    (prepared, skipped)
}

fn sanitize_string(value: Option<String>) -> Option<String> {
    value.and_then(|input| {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn sanitize_project_id(value: Option<String>) -> Option<String> {
    sanitize_string(value)
}

fn sanitize_memory_type(value: Option<String>) -> Option<String> {
    sanitize_string(value).and_then(|mut text| {
        let normalized = text.to_lowercase();
        match normalized.as_str() {
            "episodic" | "semantic" | "procedural" => {
                if text != normalized {
                    text = normalized;
                }
                Some(text)
            }
            _ => {
                tracing::warn!(memory_type = %normalized, "Ignoring unsupported memory type override");
                None
            }
        }
    })
}

fn sanitize_tags(tags: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut seen = HashSet::new();
    let mut cleaned = Vec::new();

    for tag in tags.unwrap_or_default() {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_string();
        if seen.insert(normalized.clone()) {
            cleaned.push(normalized);
        }
    }

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn map_scored_point(point: qdrant::ScoredPoint) -> SearchHit {
    let payload = point.payload.unwrap_or_default();
    let text = extract_string(&payload, "text");
    let project_id = extract_string(&payload, "project_id");
    let memory_type = extract_string(&payload, "memory_type");
    let timestamp = extract_string(&payload, "timestamp");
    let source_uri = extract_string(&payload, "source_uri");
    let tags = extract_tags(&payload);

    SearchHit {
        id: point.id,
        score: point.score,
        text,
        project_id,
        memory_type,
        tags,
        timestamp,
        source_uri,
    }
}

fn extract_string(payload: &Map<String, Value>, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn extract_tags(payload: &Map<String, Value>) -> Option<Vec<String>> {
    match payload.get("tags") {
        Some(Value::Array(values)) => {
            let tags: Vec<String> = values
                .iter()
                .filter_map(|value| value.as_str().map(|tag| tag.trim().to_string()))
                .filter(|tag| !tag.is_empty())
                .collect();
            if tags.is_empty() { None } else { Some(tags) }
        }
        Some(Value::String(tag)) => {
            let trimmed = tag.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        _ => None,
    }
}

const MIN_AUTOMATIC_CHUNK_SIZE: usize = 256;
const MAX_AUTOMATIC_CHUNK_SIZE: usize = 2048;

fn determine_chunk_size(
    override_size: Option<usize>,
    provider: EmbeddingProvider,
    model: &str,
) -> usize {
    if let Some(explicit) = override_size {
        return explicit.max(1);
    }

    // Start from the embedding model's context window so we respect its true token budget.
    let window = embedding_context_window(provider, model);
    let base = (window / 4).max(1);
    let candidate = base.max(MIN_AUTOMATIC_CHUNK_SIZE);
    // Clamp the inferred size into a friendly range for educational examples and to keep
    // retrieval latency predictable even when models support long contexts.
    candidate.clamp(MIN_AUTOMATIC_CHUNK_SIZE, MAX_AUTOMATIC_CHUNK_SIZE)
}

fn embedding_context_window(provider: EmbeddingProvider, model: &str) -> usize {
    match provider {
        EmbeddingProvider::OpenAI => openai_embedding_context_window(model),
        EmbeddingProvider::Ollama => ollama_embedding_context_window(model),
    }
}

fn openai_embedding_context_window(model: &str) -> usize {
    if model.starts_with("text-embedding-3") {
        return 8192;
    }
    if model.starts_with("text-embedding-ada-002") {
        return 8192;
    }

    let size = get_context_size(model);
    if size == 4096 && model.contains("embedding") {
        // tiktoken defaults older embedding names to 4096; call out the heuristic in case
        // readers want to swap in a more precise table later.
        tracing::debug!(model, "Using default embedding context window fallback");
    }
    size
}

fn ollama_embedding_context_window(model: &str) -> usize {
    let normalized = model.to_lowercase();
    match normalized.as_str() {
        "nomic-embed-text" | "mxbai-embed-large" | "mxbai-embed-large-v1" => 8192,
        value if value.contains("all-minilm") => 512,
        value if value.contains("e5-large") => 4096,
        _ => {
            // Ollama models do not report context sizes, so keep the fallback explicit for learners.
            tracing::trace!(model, "Using default Ollama context window estimate");
            4096
        }
    }
}

fn chunk_text(
    text: &str,
    chunk_size: usize,
    provider: EmbeddingProvider,
    model: &str,
) -> Result<Vec<String>, ChunkingError> {
    if chunk_size == 0 {
        return Err(ChunkingError::InvalidChunkSize);
    }

    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let token_counter = build_token_counter(provider, model)?;
    // Learners can swap in bespoke counters here; we keep the function small so the
    // progression from counter → chunker → embeddings is easy to follow.
    Ok(chunk_text_with_counter(text, chunk_size, token_counter))
}

fn build_token_counter(
    provider: EmbeddingProvider,
    model: &str,
) -> Result<TokenCounter, ChunkingError> {
    match provider {
        EmbeddingProvider::OpenAI => build_tiktoken_counter(model),
        EmbeddingProvider::Ollama => match build_tiktoken_counter(model) {
            Ok(counter) => Ok(counter),
            Err(error) => {
                tracing::warn!(
                    model,
                    error = %error,
                    "Tokenizer unavailable for Ollama model; falling back to whitespace counter"
                );
                Ok(default_token_counter())
            }
        },
    }
}

fn build_tiktoken_counter(model: &str) -> Result<TokenCounter, ChunkingError> {
    let normalized = model.trim();
    let target = if normalized.is_empty() {
        "cl100k_base"
    } else {
        normalized
    };
    let encoding = resolve_encoding(target).map_err(|source| ChunkingError::Tokenizer {
        model: target.to_string(),
        source,
    })?;
    let encoding = Arc::new(encoding);

    Ok(Box::new(move |segment: &str| {
        encoding.encode_ordinary(segment).len()
    }))
}

fn resolve_encoding(model: &str) -> Result<CoreBPE, TokenizerError> {
    match get_bpe_from_model(model) {
        Ok(encoding) => Ok(encoding),
        Err(model_err) => {
            tracing::debug!(model, error = %model_err, "Tokenizer model lookup failed; trying encoding name");
            if let Some(candidate) = encoding_from_name(model) {
                candidate
            } else {
                tracing::warn!(
                    model,
                    "Falling back to 'cl100k_base' encoding for token counting"
                );
                cl100k_base()
            }
        }
    }
}

fn encoding_from_name(name: &str) -> Option<Result<CoreBPE, TokenizerError>> {
    match name {
        "cl100k_base" => Some(cl100k_base()),
        "o200k_base" => Some(o200k_base()),
        "p50k_base" => Some(p50k_base()),
        "p50k_edit" => Some(p50k_edit()),
        "r50k_base" | "gpt2" => Some(r50k_base()),
        _ => None,
    }
}

fn default_token_counter() -> TokenCounter {
    Box::new(|segment: &str| {
        let tokens = segment.split_whitespace().count();
        if tokens == 0 && !segment.is_empty() {
            1
        } else {
            tokens
        }
    })
}

fn chunk_text_with_counter(
    text: &str,
    chunk_size: usize,
    token_counter: TokenCounter,
) -> Vec<String> {
    // semchunk handles the semantic splitting; feeding it the model-aware counter keeps
    // the chunk boundaries educationally relevant for retrieval demos.
    let chunker = Chunker::new(chunk_size, token_counter);
    chunker.chunk(text)
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkingError, SearchHit, build_tiktoken_counter, chunk_text, chunk_text_with_counter,
        dedupe_chunks, default_token_counter, determine_chunk_size, extract_tags, map_scored_point,
        sanitize_memory_type, sanitize_project_id, sanitize_tags,
    };
    use crate::config::EmbeddingProvider;
    use crate::qdrant::ScoredPoint;
    use serde_json::{Map, Value, json};

    #[test]
    fn chunk_text_respects_chunk_size_whitespace_counter() {
        let text = "one two three four five";
        let chunks = chunk_text_with_counter(text, 2, default_token_counter());
        assert_eq!(chunks, vec!["one two", "three four", "five"]);
    }

    #[test]
    fn chunk_text_handles_empty_input() {
        let chunks = chunk_text_with_counter("", 4, default_token_counter());
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_text_rejects_zero_chunk_size() {
        let error = chunk_text(
            "hello",
            0,
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
        )
        .unwrap_err();
        assert!(matches!(error, ChunkingError::InvalidChunkSize));
    }

    #[test]
    fn chunk_text_uses_tiktoken_budget() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let chunks = chunk_text(text, 5, EmbeddingProvider::OpenAI, "text-embedding-3-small")
            .expect("chunking succeeded");
        let token_counter = build_tiktoken_counter("text-embedding-3-small").unwrap();
        for chunk in &chunks {
            assert!(token_counter.as_ref()(chunk) <= 5);
        }
        let chunk_words: Vec<String> = chunks
            .iter()
            .flat_map(|chunk| chunk.split_whitespace().map(|word| word.to_string()))
            .collect();
        let original_words: Vec<String> = text
            .split_whitespace()
            .map(|word| word.to_string())
            .collect();
        assert_eq!(chunk_words, original_words);
    }

    #[test]
    fn determine_chunk_size_prefers_override() {
        let chunk_size = determine_chunk_size(
            Some(42),
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
        );
        assert_eq!(chunk_size, 42);
    }

    #[test]
    fn determine_chunk_size_infers_openai_embedding_window() {
        let chunk_size =
            determine_chunk_size(None, EmbeddingProvider::OpenAI, "text-embedding-3-small");
        assert_eq!(chunk_size, 2048);
    }

    #[test]
    fn determine_chunk_size_handles_common_ollama_models() {
        let chunk_size = determine_chunk_size(None, EmbeddingProvider::Ollama, "nomic-embed-text");
        assert_eq!(chunk_size, 2048);

        let mini_chunk = determine_chunk_size(None, EmbeddingProvider::Ollama, "all-minilm-l6-v2");
        assert_eq!(mini_chunk, 256);
    }

    #[test]
    fn dedupe_chunks_removes_duplicates_and_counts_skips() {
        let chunks = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
        ];
        let (deduped, skipped) = dedupe_chunks(chunks);
        let texts: Vec<_> = deduped.iter().map(|chunk| chunk.text.as_str()).collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(skipped, 2);
        assert!(texts.contains(&"alpha"));
        assert!(texts.contains(&"beta"));
        assert_ne!(deduped[0].chunk_hash, deduped[1].chunk_hash);
    }

    #[test]
    fn sanitize_memory_type_normalizes_and_filters_invalid() {
        let episodic = sanitize_memory_type(Some("Episodic".into()));
        assert_eq!(episodic.as_deref(), Some("episodic"));
        let invalid = sanitize_memory_type(Some("unknown".into()));
        assert!(invalid.is_none());
    }

    #[test]
    fn sanitize_project_id_trims_and_drops_empty() {
        assert_eq!(
            sanitize_project_id(Some("  proj  ".into())),
            Some("proj".into())
        );
        assert!(sanitize_project_id(Some("   ".into())).is_none());
    }

    #[test]
    fn sanitize_tags_uniquifies_and_trims() {
        let tags = sanitize_tags(Some(vec![
            "alpha".into(),
            " beta".into(),
            "alpha".into(),
            "".into(),
        ]));
        assert_eq!(tags.as_ref().map(|t| t.len()), Some(2));
        let values = tags.unwrap();
        assert!(values.contains(&"alpha".into()));
        assert!(values.contains(&"beta".into()));
    }

    #[test]
    fn map_scored_point_extracts_payload_fields() {
        let mut payload = Map::new();
        payload.insert("text".into(), Value::String("Example".into()));
        payload.insert("project_id".into(), Value::String("repo-a".into()));
        payload.insert("memory_type".into(), Value::String("semantic".into()));
        payload.insert(
            "timestamp".into(),
            Value::String("2025-01-01T00:00:00Z".into()),
        );
        payload.insert("source_uri".into(), Value::String("file://note".into()));
        payload.insert(
            "tags".into(),
            Value::Array(vec![
                Value::String("alpha".into()),
                Value::String("beta".into()),
            ]),
        );

        let point = ScoredPoint {
            id: "memory-1".into(),
            score: 0.42,
            payload: Some(payload),
        };

        let hit: SearchHit = map_scored_point(point);
        assert_eq!(hit.id, "memory-1");
        assert!((hit.score - 0.42).abs() < f32::EPSILON);
        assert_eq!(hit.text.as_deref(), Some("Example"));
        assert_eq!(hit.project_id.as_deref(), Some("repo-a"));
        assert_eq!(hit.memory_type.as_deref(), Some("semantic"));
        assert_eq!(hit.timestamp.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(hit.source_uri.as_deref(), Some("file://note"));
        let tags = hit.tags.expect("tags present");
        assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn extract_tags_handles_string_value() {
        let mut payload = Map::new();
        payload.insert("tags".into(), Value::String(" single ".into()));
        let tags = extract_tags(&payload).expect("tags");
        assert_eq!(tags, vec!["single".to_string()]);

        payload.insert("tags".into(), json!(["alpha", "", "beta"]));
        let tags = extract_tags(&payload).expect("array tags");
        assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);
    }
}
