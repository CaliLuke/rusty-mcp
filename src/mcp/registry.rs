use std::{collections::HashMap, future::Future, pin::Pin};

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ReadResourceRequestParam, ReadResourceResult,
};

use super::server::RustyMemMcpServer;

pub type ResourceFuture =
    Pin<Box<dyn Future<Output = Result<ReadResourceResult, McpError>> + Send>>;
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send>>;

pub type ResourceHandler = fn(&RustyMemMcpServer, ReadResourceRequestParam) -> ResourceFuture;
pub type ToolHandler = fn(&RustyMemMcpServer, CallToolRequestParam) -> ToolFuture;

/// Registry mapping resource URIs and tool names to handler functions.
pub struct Registry {
    pub resources: HashMap<&'static str, ResourceHandler>,
    pub tools: HashMap<&'static str, ToolHandler>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
            tools: HashMap::new(),
        }
    }

    pub fn register_resource(&mut self, uri: &'static str, handler: ResourceHandler) {
        self.resources.insert(uri, handler);
    }

    pub fn register_tool(&mut self, name: &'static str, handler: ToolHandler) {
        self.tools.insert(name, handler);
    }
}
