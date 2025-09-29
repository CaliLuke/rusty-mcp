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
    summarization::{SummarizationRequest as LlmSummarizationRequest, get_summarization_client},
};
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::Arc;

use super::summarize::{
    EpisodicMemory, build_abstractive_prompt, build_extractive_summary, compute_summary_key,
    sort_memories,
};
use super::types::SearchTimeRange as ProcSearchTimeRange;

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

    /// Summarize episodic (or chosen type) memories within a time range, persist a semantic summary, and return provenance.
    pub async fn summarize_memories(
        &self,
        request: SummarizeRequest,
    ) -> Result<SummarizeOutcome, SummarizeError> {
        let config = get_config();
        let collection = request
            .collection
            .clone()
            .unwrap_or_else(|| config.qdrant_collection_name.clone());

        // Validate time range
        if request.time_range.start.is_none() || request.time_range.end.is_none() {
            return Err(SummarizeError::InvalidTimeRange);
        }

        // Build episodic filter
        let filter_args = qdrant::SearchFilterArgs {
            project_id: request.project_id.clone(),
            memory_type: request
                .memory_type
                .clone()
                .or_else(|| Some("episodic".into())),
            tags: request.tags.clone(),
            time_range: Some(qdrant::SearchTimeRange {
                start: request.time_range.start.clone(),
                end: request.time_range.end.clone(),
            }),
        };
        let filter = qdrant::build_search_filter(&filter_args);

        // Scroll payloads (id + payload) and map into episodic items
        let fields = serde_json::json!(["text", "timestamp"]);
        let mut items = self
            .qdrant_service
            .scroll_payloads_with_ids(&collection, fields, filter)
            .await
            .map_err(SummarizeError::Qdrant)?
            .into_iter()
            .filter_map(|(id, payload)| {
                let text = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let timestamp = payload
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if text.trim().is_empty() {
                    None
                } else {
                    Some(EpisodicMemory::new(id, text, timestamp))
                }
            })
            .collect::<Vec<_>>();

        // Sort chronologically and cap by limit
        sort_memories(&mut items);
        let limit = request.limit.unwrap_or(50);
        if items.len() > limit {
            items.truncate(limit);
        }

        if items.is_empty() {
            return Err(SummarizeError::EmptyResult);
        }

        let source_memory_ids: Vec<String> = items.iter().map(|m| m.memory_id.clone()).collect();
        let summary_key = compute_summary_key(
            request.project_id.as_deref().unwrap_or("default"),
            &ProcSearchTimeRange {
                start: request.time_range.start.clone(),
                end: request.time_range.end.clone(),
            },
            &source_memory_ids,
        );

        // Idempotency: check for existing summary via tag summary:<hash>
        let idempotency_tag = format!("summary:{summary_key}");
        let existing_filter = qdrant::build_search_filter(&qdrant::SearchFilterArgs {
            project_id: request.project_id.clone(),
            memory_type: Some("semantic".into()),
            tags: Some(vec![idempotency_tag.clone()]),
            time_range: None,
        });
        let existing = self
            .qdrant_service
            .scroll_payloads_with_ids(&collection, serde_json::json!(["text"]), existing_filter)
            .await
            .map_err(SummarizeError::Qdrant)?;
        if let Some((existing_id, payload)) = existing.into_iter().next() {
            let summary_text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(SummarizeOutcome {
                summary: summary_text,
                source_memory_ids,
                upserted_memory_id: existing_id,
                strategy_used: strategy_to_label(&request.strategy),
                provider: request.provider,
                model: request.model,
            });
        }

        // Choose summarization strategy
        let mut chosen_strategy = request.strategy.clone().unwrap_or(SummarizeStrategy::Auto);
        let mut provider_str = request.provider.clone();
        let mut model_str = request.model.clone();

        let mut summary_text = String::new();
        if matches!(
            chosen_strategy,
            SummarizeStrategy::Auto | SummarizeStrategy::Abstractive
        ) {
            // Try abstractive path if provider active
            if matches!(
                config.summarization_provider,
                crate::config::SummarizationProvider::Ollama
            ) {
                if model_str.is_none() {
                    model_str = config.summarization_model.clone();
                }
                if provider_str.is_none() {
                    provider_str = Some("ollama".into());
                }
                if let Some(model) = model_str.clone() {
                    if let Some(client) = get_summarization_client() {
                        let prompt = build_abstractive_prompt(
                            request.project_id.as_deref().unwrap_or("default"),
                            &ProcSearchTimeRange {
                                start: request.time_range.start.clone(),
                                end: request.time_range.end.clone(),
                            },
                            request.max_words.unwrap_or(config.summarization_max_words),
                            &items,
                        );
                        match client
                            .generate_summary(LlmSummarizationRequest {
                                model,
                                prompt,
                                max_words: request
                                    .max_words
                                    .unwrap_or(config.summarization_max_words),
                            })
                            .await
                        {
                            Ok(text) => {
                                summary_text = text;
                                chosen_strategy = SummarizeStrategy::Abstractive;
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "Abstractive summarization failed; falling back to extractive");
                            }
                        }
                    }
                }
            }
        }

        // Extractive fallback or selection
        if summary_text.is_empty() {
            summary_text = build_extractive_summary(
                &items,
                request.max_words.unwrap_or(config.summarization_max_words),
            );
            if matches!(chosen_strategy, SummarizeStrategy::Auto) {
                chosen_strategy = SummarizeStrategy::Extractive;
            }
        }

        // Embed and upsert the summary as semantic
        let vectors = self
            .embedding_client
            .generate_embeddings(vec![summary_text.clone()])
            .await
            .map_err(SummarizeError::Embedding)?;
        let vector = vectors.into_iter().next().ok_or_else(|| {
            SummarizeError::Embedding(crate::embedding::EmbeddingClientError::Configuration(
                "no embedding generated".into(),
            ))
        })?;

        let chunk_hash = qdrant::compute_chunk_hash(&summary_text);
        let mut tags = request.tags.clone().unwrap_or_default();
        tags.push("summary".into());
        tags.push(format!("summary:{summary_key}"));

        let overrides = qdrant::types::PayloadOverrides {
            project_id: request.project_id.clone(),
            memory_type: Some("semantic".into()),
            tags: Some(tags),
            source_uri: None,
            source_memory_ids: Some(source_memory_ids.clone()),
            summary_key: Some(summary_key.clone()),
        };

        self.ensure_collection(&collection)
            .await
            .map_err(|e| match e {
                ProcessingError::Qdrant(err) => SummarizeError::Qdrant(err),
                ProcessingError::Embedding(err) => SummarizeError::Embedding(err),
                ProcessingError::Chunking(err) => {
                    SummarizeError::GenerationFailed(format!("chunking failed: {err}"))
                }
            })?;

        self.qdrant_service
            .index_points(
                &collection,
                vec![PointInsert {
                    text: summary_text.clone(),
                    chunk_hash,
                    vector,
                }],
                &overrides,
            )
            .await
            .map_err(SummarizeError::Qdrant)?;

        // Resolve ID of the inserted summary by scanning for the idempotency tag
        let resolve = self
            .qdrant_service
            .scroll_payloads_with_ids(
                &collection,
                serde_json::json!(["text"]),
                qdrant::build_search_filter(&qdrant::SearchFilterArgs {
                    project_id: request.project_id.clone(),
                    memory_type: Some("semantic".into()),
                    tags: Some(vec![idempotency_tag.clone()]),
                    time_range: None,
                }),
            )
            .await
            .map_err(SummarizeError::Qdrant)?;
        let upserted_memory_id = resolve
            .into_iter()
            .map(|(id, _)| id)
            .next()
            .unwrap_or_default();

        Ok(SummarizeOutcome {
            summary: summary_text,
            source_memory_ids,
            upserted_memory_id,
            strategy_used: strategy_to_label(&Some(chosen_strategy)),
            provider: provider_str,
            model: model_str,
        })
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

/// Strategy selection for summarization.
#[derive(Clone, Debug)]
pub(crate) enum SummarizeStrategy {
    /// Choose abstractive when provider available, else extractive.
    Auto,
    /// Use local LLM via provider.
    Abstractive,
    /// Deterministic bullet extraction.
    Extractive,
}

/// Input parameters for summarization.
#[derive(Clone, Debug)]
pub(crate) struct SummarizeRequest {
    pub project_id: Option<String>,
    pub memory_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub time_range: ProcSearchTimeRange,
    pub limit: Option<usize>,
    pub strategy: Option<SummarizeStrategy>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_words: Option<usize>,
    pub collection: Option<String>,
}

/// Errors surfaced from the summarization pipeline.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SummarizeError {
    #[error("Summarization provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("Failed to generate summary: {0}")]
    GenerationFailed(String),
    #[error("No episodic memories found for the requested scope")]
    EmptyResult,
    #[error("`time_range` must include both `start` and `end`")]
    InvalidTimeRange,
    #[error(transparent)]
    Embedding(#[from] crate::embedding::EmbeddingClientError),
    #[error(transparent)]
    Qdrant(#[from] crate::qdrant::types::QdrantError),
}

/// Result of a summarization request.
#[derive(Clone, Debug)]
pub(crate) struct SummarizeOutcome {
    pub summary: String,
    pub source_memory_ids: Vec<String>,
    pub upserted_memory_id: String,
    pub strategy_used: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

fn strategy_to_label(strategy: &Option<SummarizeStrategy>) -> String {
    match strategy {
        Some(SummarizeStrategy::Abstractive) => "abstractive".into(),
        Some(SummarizeStrategy::Extractive) => "extractive".into(),
        _ => "auto".into(),
    }
}
