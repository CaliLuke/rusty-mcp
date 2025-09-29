//! MCP handler for document ingestion tools.

use std::sync::Arc;

use crate::{
    config::get_config,
    processing::{IngestMetadata, ProcessingService},
};
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, JsonObject},
};
use serde::Deserialize;
use serde_json::json;

use super::parse_arguments;

/// Request payload accepted by the `push` tool.
#[derive(Debug, Deserialize)]
pub(crate) struct IndexToolRequest {
    /// Raw document text to ingest.
    pub(crate) text: String,
    /// Optional Qdrant collection override.
    #[serde(default)]
    pub(crate) collection: Option<String>,
    /// Optional project identifier persisted alongside the payload.
    #[serde(default)]
    pub(crate) project_id: Option<String>,
    /// Optional memory classification for downstream filtering.
    #[serde(default)]
    pub(crate) memory_type: Option<String>,
    /// Optional tag list applied to every chunk from the document.
    #[serde(default)]
    pub(crate) tags: Option<Vec<String>>,
    /// Optional URI describing the source document.
    #[serde(default)]
    pub(crate) source_uri: Option<String>,
}

/// Handle the `push` tool by chunking, embedding, and indexing the supplied text.
pub(crate) async fn handle_push(
    processing: &Arc<ProcessingService>,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, McpError> {
    let args: IndexToolRequest = parse_arguments(arguments)?;
    if args.text.trim().is_empty() {
        return Err(McpError::invalid_params("`text` must not be empty", None));
    }

    let IndexToolRequest {
        text,
        collection,
        project_id,
        memory_type,
        tags,
        source_uri,
    } = args;

    let collection = collection.unwrap_or_else(|| get_config().qdrant_collection_name.clone());
    let metadata = IngestMetadata {
        project_id,
        memory_type,
        tags,
        source_uri,
    };

    let outcome = processing
        .process_and_index(&collection, text, metadata)
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?;

    Ok(CallToolResult::structured(json!({
        "status": "ok",
        "collection": collection,
        "chunksIndexed": outcome.chunk_count,
        "chunkSize": outcome.chunk_size,
        "inserted": outcome.inserted,
        "updated": outcome.updated,
        "skippedDuplicates": outcome.skipped_duplicates,
    })))
}
