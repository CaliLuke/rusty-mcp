//! Document processing pipeline: chunking, embedding, and Qdrant orchestration.

pub mod chunking;
mod mappers;
pub mod sanitize;
mod service;
mod summarize;
pub mod types;

pub use service::{ProcessingApi, ProcessingService};
pub use types::{
    ChunkingError, IngestMetadata, ProcessingError, ProcessingOutcome, QdrantHealthSnapshot,
    SearchError, SearchHit, SearchRequest, SearchTimeRange,
};
// Summarization API surface re-exported for MCP (types only)
pub(crate) use service::{SummarizeError, SummarizeOutcome, SummarizeRequest, SummarizeStrategy};
