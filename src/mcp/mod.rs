//! Model Context Protocol (MCP) integration for Rusty Memory.
//!
//! This module wires the processing pipeline into an MCP server so editors and agent hosts can
//! index and search memories over stdio. The surface area consists of:
//!
//! - Tools: `push` (index), `search`, `get-collections`, `new-collection`, and `metrics`.
//! - Resources: `mcp://rusty-mem/memory-types`, `mcp://rusty-mem/health`,
//!   `mcp://rusty-mem/projects`, and a templated `mcp://rusty-mem/projects/{project_id}/tags`.
//!
//! Handlers, schemas, and formatting helpers are kept in focused submodules to make tests and
//! reviews small and targeted.

mod format;
pub mod handlers;
mod schemas;
mod server;

pub use server::RustyMemMcpServer;

/// Valid memory type values accepted by ingestion and search endpoints.
pub(crate) const MEMORY_TYPES: [&str; 3] = ["episodic", "semantic", "procedural"];
