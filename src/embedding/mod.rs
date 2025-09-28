use crate::config::{EmbeddingProvider, get_config};
use async_trait::async_trait;
use ollama_rs::Ollama;
use ollama_rs::generation::embeddings::request::GenerateEmbeddingsRequest;
use thiserror::Error;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Errors raised by embedding providers.
#[derive(Debug, Error)]
pub enum EmbeddingClientError {
    /// Provider was unable to produce embeddings for the supplied input.
    #[error("Failed to generate embeddings: {0}")]
    GenerationFailed(String),
    /// Provider was unreachable or returned a transport-level failure.
    #[error("Embedding provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// Configuration is invalid or insufficient to request embeddings.
    #[error("Invalid embedding configuration: {0}")]
    Configuration(String),
}

/// Interface implemented by embedding backends.
#[async_trait]
pub trait EmbeddingClient {
    /// Produce an embedding vector for each supplied chunk of text.
    async fn generate_embeddings(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, EmbeddingClientError>;
}

/// Deterministic fallback embedding client backed by ai-lib settings.
pub struct AiLibClient;

impl AiLibClient {
    /// Construct a new deterministic embedding client instance.
    pub const fn new() -> Self {
        Self
    }

    fn encode(text: &str, dimension: usize) -> Vec<f32> {
        let mut embedding = vec![0.0_f32; dimension];

        if text.is_empty() {
            return embedding;
        }

        for (idx, byte) in text.bytes().enumerate() {
            let position = idx % dimension;
            // Basic hashing of content into the vector slot
            embedding[position] += f32::from(byte) / 255.0;
        }

        let norm = embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();

        if norm > 0.0 {
            for value in &mut embedding {
                *value /= norm;
            }
        }

        embedding
    }
}

impl Default for AiLibClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EmbeddingClient for AiLibClient {
    async fn generate_embeddings(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, EmbeddingClientError> {
        let config = get_config();
        let dimension = config.embedding_dimension;

        tracing::debug!(
            provider = ?config.embedding_provider,
            model = %config.embedding_model,
            dimension,
            "Generating embeddings"
        );

        if dimension == 0 {
            return Err(EmbeddingClientError::Configuration(
                "embedding dimension must be greater than zero".to_string(),
            ));
        }

        if texts.is_empty() {
            return Err(EmbeddingClientError::Configuration(
                "no texts provided".to_string(),
            ));
        }

        let embeddings = texts
            .into_iter()
            .map(|text| Self::encode(&text, dimension))
            .collect();

        Ok(embeddings)
    }
}

#[derive(Clone)]
struct OllamaClient {
    inner: Ollama,
    model: String,
    dimension: usize,
    base_url: String,
}

impl OllamaClient {
    fn try_new(
        base_url: String,
        model: String,
        dimension: usize,
    ) -> Result<Self, EmbeddingClientError> {
        if dimension == 0 {
            return Err(EmbeddingClientError::Configuration(
                "embedding dimension must be greater than zero".to_string(),
            ));
        }

        let inner = Ollama::try_new(base_url.as_str()).map_err(|error| {
            EmbeddingClientError::Configuration(format!("invalid OLLAMA_URL '{base_url}': {error}"))
        })?;

        Ok(Self {
            inner,
            model,
            dimension,
            base_url,
        })
    }
}

#[async_trait]
impl EmbeddingClient for OllamaClient {
    async fn generate_embeddings(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, EmbeddingClientError> {
        if texts.is_empty() {
            return Err(EmbeddingClientError::Configuration(
                "no texts provided".to_string(),
            ));
        }

        let text_count = texts.len();

        tracing::debug!(
            url = %self.base_url,
            model = %self.model,
            count = text_count,
            "Requesting embeddings from Ollama",
        );

        let request = GenerateEmbeddingsRequest::new(self.model.clone(), texts.into());
        let response = self
            .inner
            .generate_embeddings(request)
            .await
            .map_err(|error| {
                EmbeddingClientError::ProviderUnavailable(format!(
                    "failed to reach Ollama at {}: {}. Set OLLAMA_URL and ensure the runtime is running.",
                    self.base_url, error
                ))
            })?;

        let embeddings = response.embeddings;

        if embeddings.len() != text_count {
            return Err(EmbeddingClientError::GenerationFailed(format!(
                "Ollama at {} returned {} embeddings for {} texts",
                self.base_url,
                embeddings.len(),
                text_count
            )));
        }

        for vector in &embeddings {
            if vector.len() != self.dimension {
                return Err(EmbeddingClientError::GenerationFailed(format!(
                    "Ollama model '{}' at {} produced vectors of dimension {} but EMBEDDING_DIMENSION is {}. Update EMBEDDING_DIMENSION or use a compatible model.",
                    self.model,
                    self.base_url,
                    vector.len(),
                    self.dimension
                )));
            }
        }

        Ok(embeddings)
    }
}

/// Build an embedding client suitable for the current configuration.
pub fn get_embedding_client() -> Box<dyn EmbeddingClient + Send + Sync> {
    let config = get_config();
    match config.embedding_provider {
        EmbeddingProvider::Ollama => {
            let base_url = config
                .ollama_url
                .clone()
                .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string());
            tracing::info!(
                provider = "ollama",
                url = %base_url,
                model = %config.embedding_model,
                "Using Ollama embedding provider"
            );
            let client = OllamaClient::try_new(
                base_url,
                config.embedding_model.clone(),
                config.embedding_dimension,
            )
            .unwrap_or_else(|error| {
                panic!("Failed to initialize Ollama embedding client: {error}");
            });
            Box::new(client)
        }
        EmbeddingProvider::OpenAI => {
            tracing::info!(
                provider = "deterministic-fallback",
                configured_provider = ?config.embedding_provider,
                "Using deterministic embeddings for compatibility"
            );
            Box::new(AiLibClient::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EmbeddingClientError, OllamaClient};

    #[test]
    fn ollama_client_rejects_zero_dimension() {
        let result = OllamaClient::try_new(
            "http://localhost:11434".to_string(),
            "test-model".to_string(),
            0,
        );

        assert!(matches!(
            result,
            Err(EmbeddingClientError::Configuration(message))
                if message.contains("dimension must be greater than zero")
        ));
    }

    #[test]
    fn ollama_client_requires_valid_url() {
        let result = OllamaClient::try_new("not a url".to_string(), "test-model".to_string(), 128);

        assert!(
            matches!(result, Err(EmbeddingClientError::Configuration(message)) if message.contains("invalid OLLAMA_URL"))
        );
    }
}
