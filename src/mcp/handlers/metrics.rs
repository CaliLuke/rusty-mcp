//! Handler for the metrics tool.

use std::sync::Arc;

use crate::processing::ProcessingService;
use rmcp::{ErrorData as McpError, model::CallToolResult};
use serde_json::json;

/// Handle the `metrics` tool, returning the current ingestion counters.
pub(crate) async fn handle_metrics(
    processing: &Arc<ProcessingService>,
) -> Result<CallToolResult, McpError> {
    let snapshot = processing.metrics_snapshot();
    Ok(CallToolResult::structured(json!({
        "documentsIndexed": snapshot.documents_indexed,
        "chunksIndexed": snapshot.chunks_indexed,
        "lastChunkSize": snapshot.last_chunk_size,
    })))
}
