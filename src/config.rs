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
        "Loaded configuration"
    );
    CONFIG.set(config).expect("Failed to set config");
}
