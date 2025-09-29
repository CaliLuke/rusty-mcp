//! Lightweight ingestion counters used for diagnostics.
//!
//! The `CodeMetrics` type exposes lock‑free counters that track:
//! - Documents indexed
//! - Chunks indexed (cumulative)
//! - The effective chunk size used for the last ingestion
//!
//! The snapshot is surfaced via HTTP (`GET /metrics`) and MCP (`metrics` tool) to help validate
//! chunking heuristics and overall ingestion activity during development.

use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe counters describing ingestion activity.
///
/// The struct intentionally stays minimal—just atomic counters—so it can be cloned freely and
/// queried without holding locks.  The metrics surface already exposes the most recent chunk size
/// so front-ends can teach how the automatic sizing behaves over time.
#[derive(Default)]
pub struct CodeMetrics {
    documents_indexed: AtomicU64,
    chunks_indexed: AtomicU64,
    last_chunk_size: AtomicU64,
}

impl CodeMetrics {
    /// Create an empty metrics accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a processed document and the number of chunks produced for it.
    ///
    /// The caller supplies the number of chunks and the chunk size used for the ingestion.  We
    /// capture the chunk size so diagnostics can show how the automatic heuristics evolve when
    /// different embedding models are configured.
    pub fn record_document(&self, chunk_count: u64, chunk_size: u64) {
        self.documents_indexed.fetch_add(1, Ordering::Relaxed);
        self.chunks_indexed
            .fetch_add(chunk_count, Ordering::Relaxed);
        // Persist the effective chunk size so the dashboard endpoints can explain
        // how the automatic sizing behaved for the last ingestion.
        self.last_chunk_size.store(chunk_size, Ordering::Relaxed);
    }

    /// Return a snapshot of the current counters.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            documents_indexed: self.documents_indexed.load(Ordering::Relaxed),
            chunks_indexed: self.chunks_indexed.load(Ordering::Relaxed),
            last_chunk_size: {
                let documents = self.documents_indexed.load(Ordering::Relaxed);
                let last = self.last_chunk_size.load(Ordering::Relaxed);
                if documents == 0 || last == 0 {
                    None
                } else {
                    // Expose a value only after the first document to avoid confusing
                    // consumers with a meaningless default.
                    Some(last)
                }
            },
        }
    }
}

/// Immutable view of ingestion counters used for reporting.
///
/// Exposed through both the HTTP `/metrics` endpoint and the MCP `metrics` tool so that editors
/// and dashboards can display ingestion activity without depending on interior mutability.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct MetricsSnapshot {
    /// Number of documents that have been indexed since startup.
    pub documents_indexed: u64,
    /// Total chunk count produced across all indexed documents.
    pub chunks_indexed: u64,
    /// Chunk size used for the most recently ingested document, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_chunk_size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_documents_and_chunks() {
        let metrics = CodeMetrics::new();
        metrics.record_document(2, 128);
        metrics.record_document(3, 256);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.documents_indexed, 2);
        assert_eq!(snapshot.chunks_indexed, 5);
        assert_eq!(snapshot.last_chunk_size, Some(256));
    }

    #[test]
    fn snapshot_is_consistent() {
        let metrics = CodeMetrics::new();
        assert_eq!(metrics.snapshot().documents_indexed, 0);
        assert_eq!(metrics.snapshot().chunks_indexed, 0);
        assert_eq!(metrics.snapshot().last_chunk_size, None);
    }
}
