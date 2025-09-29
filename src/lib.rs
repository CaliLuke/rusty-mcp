#![deny(missing_docs)]
#![forbid(unsafe_code)]

//! Rusty Memory is an educational memory server that demonstrates how to wire
//! semantic chunking, embeddings, and Qdrant together behind both an HTTP API
//! and the Model Context Protocol.  The crate exposes deliberate, well-commented
//! modules so learners can trace the end-to-end data flow:
//!
//! * `config` loads the runtime configuration and explains the required
//!   environment variables.
//! * `processing` contains the orchestration pipeline that chunks text, requests
//!   embeddings, and writes vectors to Qdrant while recording metrics.
//! * `api` and `mcp` surface the same processing primitives through REST and MCP
//!   tooling respectively.
//! * `metrics`, `logging`, and `qdrant` provide the supporting infrastructure.
//!
//! The library can be embedded in other projects or used as documentation for
//! students exploring how modern agent memory stacks are built in Rust.

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
/// Optional abstractive summarization client(s).
pub mod summarization;
