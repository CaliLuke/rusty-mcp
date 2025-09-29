//! Handlers for collection discovery and management tools.

use std::sync::Arc;

use crate::{config::get_config, processing::ProcessingService};
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, JsonObject},
};
use serde::Deserialize;
use serde_json::json;

use super::parse_arguments;

/// Request payload for the `new-collection` tool.
#[derive(Debug, Deserialize)]
pub(crate) struct CreateCollectionRequest {
    /// Name of the Qdrant collection to ensure.
    pub(crate) name: String,
    /// Optional vector dimension override.
    #[serde(default)]
    pub(crate) vector_size: Option<u64>,
}

/// Handle the `get-collections` tool, returning known Qdrant collections.
pub(crate) async fn handle_list_collections(
    processing: &Arc<ProcessingService>,
) -> Result<CallToolResult, McpError> {
    let collections = processing
        .list_collections()
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?;
    Ok(CallToolResult::structured(
        json!({ "collections": collections }),
    ))
}

/// Handle the `new-collection` tool by ensuring a collection exists with the desired size.
pub(crate) async fn handle_create_collection(
    processing: &Arc<ProcessingService>,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, McpError> {
    let args: CreateCollectionRequest = parse_arguments(arguments)?;
    if args.name.trim().is_empty() {
        return Err(McpError::invalid_params("`name` must not be empty", None));
    }

    let target_size = args.vector_size.unwrap_or_else(|| {
        let cfg = get_config();
        cfg.embedding_dimension as u64
    });

    processing
        .create_collection(&args.name, Some(target_size))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?;

    Ok(CallToolResult::structured(json!({
        "status": "ok",
        "vectorSize": target_size,
    })))
}
