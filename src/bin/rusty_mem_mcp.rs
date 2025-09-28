use anyhow::{Context, Result};
use rmcp::{service::ServiceExt, transport::stdio};
use rustymcp::{config, logging, mcp::RustyMemMcpServer, processing};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    config::init_config();
    logging::init_tracing();

    let processing = Arc::new(processing::ProcessingService::new().await);
    let server = RustyMemMcpServer::new(processing);

    let service = server
        .serve(stdio())
        .await
        .context("failed to start MCP server over stdio")?;

    service
        .waiting()
        .await
        .context("MCP server terminated unexpectedly")?;

    Ok(())
}
