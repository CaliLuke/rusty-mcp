//! Environment-driven configuration for Rusty Memory.
//!
//! This module loads and validates settings once at startup (via `init_config`) and exposes
//! a global, read‑only view through `get_config`. The configuration powers both the HTTP API and
//! the MCP server and includes:
//!
//! - Qdrant connectivity (`QDRANT_URL`, `QDRANT_COLLECTION_NAME`, `QDRANT_API_KEY?`).
//! - Embedding provider/model (`EMBEDDING_PROVIDER`, `EMBEDDING_MODEL`, `EMBEDDING_DIMENSION`,
//!   `OLLAMA_URL?`).
//! - Chunking overrides (`TEXT_SPLITTER_CHUNK_SIZE?`, `TEXT_SPLITTER_CHUNK_OVERLAP?`,
//!   `TEXT_SPLITTER_USE_SAFE_DEFAULTS?`).
//! - Search ergonomics (`SEARCH_DEFAULT_LIMIT?`, `SEARCH_MAX_LIMIT?`,
//!   `SEARCH_DEFAULT_SCORE_THRESHOLD?`).
//! - Summarization (`SUMMARIZATION_PROVIDER?`, `SUMMARIZATION_MODEL?`,
//!   `SUMMARIZATION_MAX_WORDS?`).
//! - HTTP server port (`SERVER_PORT?`).
//!
//! Most fields are optional with sensible defaults; invalid combinations are flagged early with
//! descriptive errors so misconfiguration is easy to diagnose.
use serde::Deserialize;
use std::env;
use std::sync::OnceLock;
use thiserror::Error;

/// Errors encountered while loading configuration from environment variables.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Required environment variable was not provided.
    #[error("Missing environment variable: {0}")]
    MissingVariable(String),
    /// Environment variable contained a value that could not be parsed.
    #[error("Invalid value for environment variable: {0}")]
    InvalidValue(String),
}

/// Runtime configuration for the Rusty Memory server.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Base URL of the Qdrant instance that stores embeddings.
    pub qdrant_url: String,
    /// Name of the Qdrant collection used for document storage.
    pub qdrant_collection_name: String,
    /// Optional API key required to access Qdrant.
    pub qdrant_api_key: Option<String>,
    /// Embedding provider used to generate vector representations.
    pub embedding_provider: EmbeddingProvider,
    /// Optional override for the automatic chunk size selection.
    pub text_splitter_chunk_size: Option<usize>,
    /// Optional overlap between sequential chunks produced by the splitter.
    pub text_splitter_chunk_overlap: Option<usize>,
    /// Opt-in flag enabling safer chunk-size defaults tuned for retrieval quality.
    pub text_splitter_use_safe_defaults: bool,
    /// Embedding model identifier passed to the provider.
    pub embedding_model: String,
    /// Dimensionality of the produced vectors.
    pub embedding_dimension: usize,
    /// Base URL of the Ollama runtime providing embeddings (when enabled).
    pub ollama_url: Option<String>,
    /// Optional override for the HTTP server port.
    pub server_port: Option<u16>,
    /// Default number of results returned by search when callers omit `limit`.
    pub search_default_limit: usize,
    /// Maximum number of results allowed per search request.
    pub search_max_limit: usize,
    /// Default similarity threshold applied when callers omit `score_threshold`.
    pub search_default_score_threshold: f32,
    /// Summarization provider selection.
    pub summarization_provider: SummarizationProvider,
    /// Optional model identifier for abstractive summarization.
    pub summarization_model: Option<String>,
    /// Default word budget for summaries.
    pub summarization_max_words: usize,
}

/// Supported embedding backends for the processing pipeline.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    /// Local Ollama runtime.
    Ollama,
    /// Hosted OpenAI embeddings API.
    OpenAI,
}

/// Supported summarization backends for abstractive summaries.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummarizationProvider {
    /// Disable abstractive summarization; use extractive fallback.
    None,
    /// Local Ollama runtime.
    Ollama,
}

impl Config {
    /// Load configuration from environment variables, performing validation along the way.
    pub fn from_env() -> Result<Self, ConfigError> {
        let search_default_limit = load_usize_with_default("SEARCH_DEFAULT_LIMIT", 5)?;
        let search_max_limit = load_usize_with_default("SEARCH_MAX_LIMIT", 50)?;
        let search_default_score_threshold =
            load_f32_with_default("SEARCH_DEFAULT_SCORE_THRESHOLD", 0.25)?;

        if search_default_limit == 0 {
            return Err(ConfigError::InvalidValue(
                "SEARCH_DEFAULT_LIMIT must be at least 1".into(),
            ));
        }
        if search_max_limit == 0 {
            return Err(ConfigError::InvalidValue(
                "SEARCH_MAX_LIMIT must be at least 1".into(),
            ));
        }
        if search_default_limit > search_max_limit {
            return Err(ConfigError::InvalidValue(
                "SEARCH_DEFAULT_LIMIT cannot exceed SEARCH_MAX_LIMIT".into(),
            ));
        }
        if !(0.0..=1.0).contains(&search_default_score_threshold) {
            return Err(ConfigError::InvalidValue(
                "SEARCH_DEFAULT_SCORE_THRESHOLD must be between 0.0 and 1.0".into(),
            ));
        }

        Ok(Self {
            qdrant_url: load_env("QDRANT_URL")?,
            qdrant_collection_name: load_env("QDRANT_COLLECTION_NAME")?,
            qdrant_api_key: load_env_optional("QDRANT_API_KEY"),
            embedding_provider: load_env("EMBEDDING_PROVIDER")?.parse().map_err(|()| {
                ConfigError::MissingVariable("Invalid EMBEDDING_PROVIDER".to_string())
            })?,
            text_splitter_chunk_size: load_env_optional("TEXT_SPLITTER_CHUNK_SIZE")
                .map(|value| {
                    value.parse().map_err(|_| {
                        ConfigError::InvalidValue("TEXT_SPLITTER_CHUNK_SIZE".to_string())
                    })
                })
                .transpose()?,
            text_splitter_chunk_overlap: load_env_optional("TEXT_SPLITTER_CHUNK_OVERLAP")
                .map(|value| {
                    value.parse().map_err(|_| {
                        ConfigError::InvalidValue("TEXT_SPLITTER_CHUNK_OVERLAP".to_string())
                    })
                })
                .transpose()?,
            text_splitter_use_safe_defaults: load_bool_with_default(
                "TEXT_SPLITTER_USE_SAFE_DEFAULTS",
                false,
            )?,
            embedding_model: load_env("EMBEDDING_MODEL")?,
            embedding_dimension: load_env("EMBEDDING_DIMENSION")?.parse().map_err(|_| {
                ConfigError::MissingVariable("Invalid EMBEDDING_DIMENSION".to_string())
            })?,
            ollama_url: load_env_optional("OLLAMA_URL"),
            server_port: load_env_optional("SERVER_PORT")
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| ConfigError::InvalidValue("SERVER_PORT".into()))
                })
                .transpose()?,
            search_default_limit,
            search_max_limit,
            search_default_score_threshold,
            summarization_provider: load_env_optional("SUMMARIZATION_PROVIDER")
                .as_deref()
                .map(|s| match s.to_lowercase().as_str() {
                    "ollama" => SummarizationProvider::Ollama,
                    _ => SummarizationProvider::None,
                })
                .unwrap_or(SummarizationProvider::None),
            summarization_model: load_env_optional("SUMMARIZATION_MODEL"),
            summarization_max_words: load_usize_with_default("SUMMARIZATION_MAX_WORDS", 250)?,
        })
    }
}

fn load_usize_with_default(key: &str, default: usize) -> Result<usize, ConfigError> {
    match load_env_optional(key) {
        Some(value) => value
            .parse()
            .map_err(|_| ConfigError::InvalidValue(key.to_string())),
        None => Ok(default),
    }
}

fn load_f32_with_default(key: &str, default: f32) -> Result<f32, ConfigError> {
    match load_env_optional(key) {
        Some(value) => value
            .parse()
            .map_err(|_| ConfigError::InvalidValue(key.to_string())),
        None => Ok(default),
    }
}

fn load_bool_with_default(key: &str, default: bool) -> Result<bool, ConfigError> {
    match load_env_optional(key) {
        Some(value) => match value.to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(ConfigError::InvalidValue(key.to_string())),
        },
        None => Ok(default),
    }
}

fn load_env(key: &str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::MissingVariable(key.to_string()))
}

fn load_env_optional(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

impl std::str::FromStr for EmbeddingProvider {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::OpenAI),
            _ => Err(()),
        }
    }
}

/// Global configuration cache populated during process start.
pub static CONFIG: OnceLock<Config> = OnceLock::new();

/// Retrieve the loaded configuration, panicking if initialization has not occurred.
pub fn get_config() -> &'static Config {
    CONFIG.get().expect("Config not initialized")
}

/// Load configuration from the environment and install it in the global cache.
pub fn init_config() {
    dotenvy::dotenv().ok();
    let config = Config::from_env().expect("Failed to load config from environment");
    tracing::debug!(
        qdrant_url = %config.qdrant_url,
        collection = %config.qdrant_collection_name,
        server_port = ?config.server_port,
        embedding_provider = ?config.embedding_provider,
        ollama_url = ?config.ollama_url,
        search_default_limit = config.search_default_limit,
        search_max_limit = config.search_max_limit,
        search_default_score_threshold = config.search_default_score_threshold,
        summarization_provider = ?config.summarization_provider,
        summarization_model = ?config.summarization_model,
        summarization_max_words = config.summarization_max_words,
        "Loaded configuration"
    );
    CONFIG.set(config).expect("Failed to set config");
}
