#![deny(missing_docs)]

//! Core library for the Rusty Memory MCP server.

/// HTTP routing and REST handlers.
pub mod api;
/// Environment-driven configuration management.
pub mod config;
/// Embedding client abstraction and adapters.
pub mod embedding;
/// Structured logging and tracing setup.
pub mod logging;
/// Model Context Protocol server implementation.
pub mod mcp;
/// Ingestion metrics helpers.
pub mod metrics;
/// Document processing pipeline utilities.
pub mod processing;
/// Qdrant vector store integration.
pub mod qdrant;
