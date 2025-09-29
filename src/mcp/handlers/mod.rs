//! Tool handlers for the MCP server.

use rmcp::{ErrorData as McpError, model::JsonObject};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub mod collections;
pub mod index;
pub mod metrics;
pub mod search;

/// Parse structured arguments supplied to a tool invocation.
pub(crate) fn parse_arguments<T: DeserializeOwned>(
    arguments: Option<JsonObject>,
) -> Result<T, McpError> {
    let value = arguments
        .map(Value::Object)
        .unwrap_or_else(|| Value::Object(JsonObject::new()));
    parse_arguments_value(value)
}

/// Deserialize arguments represented as a JSON value into the target type.
pub(crate) fn parse_arguments_value<T: DeserializeOwned>(value: Value) -> Result<T, McpError> {
    serde_json::from_value(value)
        .map_err(|err| McpError::invalid_params(format!("Invalid arguments: {err}"), None))
}
