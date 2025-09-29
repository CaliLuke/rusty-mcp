//! Shared types used by the Qdrant client and helpers.

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

/// Errors returned while interacting with Qdrant.
#[derive(Debug, Error)]
pub enum QdrantError {
    /// Base URL failed to parse or normalize.
    #[error("Invalid Qdrant URL: {0}")]
    InvalidUrl(String),
    /// HTTP layer failed before receiving a response.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Qdrant responded with an unexpected status code.
    #[error("Unexpected Qdrant response ({status}): {body}")]
    UnexpectedStatus {
        /// HTTP status returned from Qdrant.
        status: StatusCode,
        /// Body payload associated with the failing response.
        body: String,
    },
}

/// Optional metadata fields propagated into each Qdrant payload.
#[derive(Debug, Clone, Default)]
pub struct PayloadOverrides {
    /// Override for the `project_id` field.
    pub project_id: Option<String>,
    /// Override for the `memory_type` field.
    pub memory_type: Option<String>,
    /// Tags attached to each chunk for filterable metadata.
    pub tags: Option<Vec<String>>,
    /// Optional URI describing the chunk source.
    pub source_uri: Option<String>,
    /// Optional provenance of episodic memories consolidated into this item.
    pub source_memory_ids: Option<Vec<String>>,
    /// Optional idempotency key for summaries.
    pub summary_key: Option<String>,
}

/// Prepared point ready for indexing, including text, hash, and vector.
#[derive(Debug, Clone)]
pub struct PointInsert {
    /// Raw chunk text.
    pub text: String,
    /// Deterministic hash of the chunk used for dedupe.
    pub chunk_hash: String,
    /// Embedding vector produced for the chunk.
    pub vector: Vec<f32>,
}

/// Filters that can be applied to Qdrant search queries.
#[derive(Debug, Default, Clone)]
pub struct SearchFilterArgs {
    /// Exact match constraint for the `project_id` payload field.
    pub project_id: Option<String>,
    /// Exact match constraint for the `memory_type` payload field.
    pub memory_type: Option<String>,
    /// Contains-any constraint for the `tags` payload field.
    pub tags: Option<Vec<String>>,
    /// Timestamp boundaries applied to the `timestamp` payload field.
    pub time_range: Option<SearchTimeRange>,
}

/// Inclusive timestamp boundaries expressed in RFC3339.
#[derive(Debug, Default, Clone)]
pub struct SearchTimeRange {
    /// Inclusive start timestamp (`gte`).
    pub start: Option<String>,
    /// Inclusive end timestamp (`lte`).
    pub end: Option<String>,
}

/// Scored payload returned by Qdrant queries.
#[derive(Debug, Clone)]
pub struct ScoredPoint {
    /// Identifier assigned to the vector.
    pub id: String,
    /// Similarity score computed by Qdrant.
    pub score: f32,
    /// Optional payload associated with the vector.
    pub payload: Option<Map<String, Value>>,
}

/// Summary describing how Qdrant applied an indexing request.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexSummary {
    /// Number of new vectors inserted by the request.
    pub inserted: usize,
    /// Number of vectors updated in place.
    pub updated: usize,
}

#[derive(Deserialize)]
pub(crate) struct ListCollectionsResponse {
    pub(crate) result: ListCollectionsResult,
}

#[derive(Deserialize)]
pub(crate) struct ListCollectionsResult {
    pub(crate) collections: Vec<CollectionDescription>,
}

#[derive(Deserialize)]
pub(crate) struct CollectionDescription {
    pub(crate) name: String,
}

#[derive(Deserialize)]
pub(crate) struct QueryResponse {
    pub(crate) result: QueryResponseResult,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum QueryResponseResult {
    Points(Vec<QueryPoint>),
    Object {
        #[serde(default)]
        points: Vec<QueryPoint>,
        #[serde(default)]
        _count: Option<usize>,
    },
}

#[derive(Deserialize)]
pub(crate) struct QueryPoint {
    pub(crate) id: Value,
    pub(crate) score: f32,
    #[serde(default)]
    pub(crate) payload: Option<Map<String, Value>>,
}

#[derive(Deserialize)]
pub(crate) struct ScrollResponse {
    pub(crate) result: ScrollResult,
}

#[derive(Deserialize)]
pub(crate) struct ScrollResult {
    #[serde(default)]
    pub(crate) points: Vec<ScrollPoint>,
    #[serde(default)]
    pub(crate) next_page_offset: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct ScrollPoint {
    #[serde(default)]
    pub(crate) id: Option<Value>,
    #[serde(default)]
    pub(crate) payload: Option<Map<String, Value>>,
}
