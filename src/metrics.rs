use std::sync::atomic::{AtomicU64, Ordering};

/// Thread-safe counters describing ingestion activity.
#[derive(Default)]
pub struct CodeMetrics {
    documents_indexed: AtomicU64,
    chunks_indexed: AtomicU64,
}

impl CodeMetrics {
    /// Create an empty metrics accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a processed document and the number of chunks produced for it.
    pub fn record_document(&self, chunk_count: u64) {
        self.documents_indexed.fetch_add(1, Ordering::Relaxed);
        self.chunks_indexed
            .fetch_add(chunk_count, Ordering::Relaxed);
    }

    /// Return a snapshot of the current counters.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            documents_indexed: self.documents_indexed.load(Ordering::Relaxed),
            chunks_indexed: self.chunks_indexed.load(Ordering::Relaxed),
        }
    }
}

/// Immutable view of ingestion counters used for reporting.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct MetricsSnapshot {
    /// Number of documents that have been indexed since startup.
    pub documents_indexed: u64,
    /// Total chunk count produced across all indexed documents.
    pub chunks_indexed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_documents_and_chunks() {
        let metrics = CodeMetrics::new();
        metrics.record_document(2);
        metrics.record_document(3);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.documents_indexed, 2);
        assert_eq!(snapshot.chunks_indexed, 5);
    }

    #[test]
    fn snapshot_is_consistent() {
        let metrics = CodeMetrics::new();
        assert_eq!(metrics.snapshot().documents_indexed, 0);
        assert_eq!(metrics.snapshot().chunks_indexed, 0);
    }
}
