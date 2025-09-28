use crate::config::{EmbeddingProvider, get_config};
use crate::embedding::{EmbeddingClient, get_embedding_client};
use crate::metrics::{CodeMetrics, MetricsSnapshot};
use crate::qdrant::{QdrantError, QdrantService};
use anyhow::Error as TokenizerError;
use semchunk_rs::Chunker;
use std::sync::Arc;
use thiserror::Error;
use tiktoken_rs::{
    CoreBPE, cl100k_base, get_bpe_from_model, model::get_context_size, o200k_base, p50k_base,
    p50k_edit, r50k_base,
};

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
    /// Qdrant rejected an indexing operation.
    #[error("Failed to index document: {0}")]
    Indexing(#[from] QdrantError),
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
    /// flushed to Qdrant in a single batch.  The structured [`ProcessingOutcome`] reports how
    /// many chunks were generated and which chunk size heuristic was applied.
    pub async fn process_and_index(
        &self,
        collection_name: &str,
        text: String,
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
        let chunk_count = chunks.len();
        let embeddings = self
            .embedding_client
            .generate_embeddings(chunks.clone())
            .await?;

        self.qdrant_service
            .index_points(collection_name, chunks, embeddings)
            .await?;

        self.metrics
            .record_document(chunk_count as u64, chunk_size as u64);
        tracing::info!(
            collection = collection_name,
            chunks = chunk_count,
            chunk_size,
            "Document indexed"
        );

        Ok(ProcessingOutcome {
            chunk_count,
            chunk_size,
        })
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
            .map_err(ProcessingError::from)
            .map(|()| {
                tracing::debug!(collection = collection_name, "Collection ensured");
            })
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
    ///
    /// Callers can expose the snapshot through diagnostics endpoints or dashboards.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
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
) -> Result<Box<dyn Fn(&str) -> usize>, ChunkingError> {
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

fn build_tiktoken_counter(model: &str) -> Result<Box<dyn Fn(&str) -> usize>, ChunkingError> {
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

fn default_token_counter() -> Box<dyn Fn(&str) -> usize> {
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
    token_counter: Box<dyn Fn(&str) -> usize>,
) -> Vec<String> {
    // semchunk handles the semantic splitting; feeding it the model-aware counter keeps
    // the chunk boundaries educationally relevant for retrieval demos.
    let chunker = Chunker::new(chunk_size, token_counter);
    chunker.chunk(text)
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkingError, build_tiktoken_counter, chunk_text, chunk_text_with_counter,
        default_token_counter, determine_chunk_size,
    };
    use crate::config::EmbeddingProvider;

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
}
