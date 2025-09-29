//! Qdrant vector store integration.

pub mod client;
pub mod filters;
pub mod payload;
/// Streaming helpers for Qdrant scroll pagination.
pub mod scroller;
pub mod types;

pub use client::QdrantService;
pub use filters::{accumulate_project_id, accumulate_tags, build_search_filter};
pub use payload::compute_chunk_hash;
pub use types::{
    IndexSummary, PayloadOverrides, PointInsert, QdrantError, ScoredPoint, SearchFilterArgs,
    SearchTimeRange,
};
