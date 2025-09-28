use crate::config::get_config;
use async_trait::async_trait;
use thiserror::Error;

/// Errors raised by embedding providers.
#[derive(Debug, Error)]
pub enum EmbeddingClientError {
    /// Provider was unable to produce embeddings for the supplied input.
    #[error("Failed to generate embeddings: {0}")]
    GenerationFailed(String),
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
            return Err(EmbeddingClientError::GenerationFailed(
                "embedding dimension must be greater than zero".to_string(),
            ));
        }

        if texts.is_empty() {
            return Err(EmbeddingClientError::GenerationFailed(
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

/// Build an embedding client suitable for the current configuration.
pub fn get_embedding_client() -> Box<dyn EmbeddingClient + Send + Sync> {
    Box::new(AiLibClient::new())
}
