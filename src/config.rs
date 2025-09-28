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
    /// Optional override for the HTTP server port.
    pub server_port: Option<u16>,
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
            server_port: load_env_optional("SERVER_PORT")
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| ConfigError::InvalidValue("SERVER_PORT".into()))
                })
                .transpose()?,
        })
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
        "Loaded configuration"
    );
    CONFIG.set(config).expect("Failed to set config");
}
