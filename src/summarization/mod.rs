//! Abstractions for generating abstractive summaries via local providers.
//!
//! The summarization pipeline is optional; when no provider is configured the processing layer
//! falls back to deterministic extractive summaries. The Ollama-backed client mirrors the
//! embedding adapter by issuing HTTP requests directly to the runtime.

use crate::config::{SummarizationProvider, get_config};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Errors surfaced while attempting abstractive summarization.
#[derive(Debug, Error)]
pub enum SummarizationClientError {
    /// Provider was explicitly disabled or unreachable.
    #[error("Summarization provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// Provider returned an error response.
    #[error("Failed to generate summary: {0}")]
    GenerationFailed(String),
    /// Provider response could not be parsed.
    #[error("Malformed provider response: {0}")]
    InvalidResponse(String),
}

/// Request payload passed to the summarization provider.
#[derive(Debug, Clone)]
pub struct SummarizationRequest {
    /// Fully qualified model identifier understood by the provider.
    pub model: String,
    /// Prompt assembled by the processing pipeline.
    pub prompt: String,
    /// Maximum word budget requested by the caller.
    pub max_words: usize,
}

/// Interface implemented by abstractive summarization providers.
#[async_trait]
pub trait SummarizationClient: Send + Sync {
    /// Generate a concise summary using the configured model.
    async fn generate_summary(
        &self,
        request: SummarizationRequest,
    ) -> Result<String, SummarizationClientError>;
}

/// Build a summarization client based on configuration.
pub fn get_summarization_client() -> Option<Box<dyn SummarizationClient + Send + Sync>> {
    let config = get_config();
    match config.summarization_provider {
        SummarizationProvider::None => None,
        SummarizationProvider::Ollama => {
            let base_url = config
                .ollama_url
                .clone()
                .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string());
            Some(Box::new(OllamaSummarizationClient::new(base_url)))
        }
    }
}

struct OllamaSummarizationClient {
    http: Client,
    base_url: String,
}

impl OllamaSummarizationClient {
    fn new(base_url: String) -> Self {
        let http = Client::builder()
            .user_agent("rusty-mem/summary")
            .build()
            .expect("Failed to construct reqwest::Client for summarization");
        Self { http, base_url }
    }

    fn endpoint(&self) -> String {
        format!("{}/api/generate", self.base_url.trim_end_matches('/'))
    }
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
    done: bool,
}

#[async_trait]
impl SummarizationClient for OllamaSummarizationClient {
    async fn generate_summary(
        &self,
        request: SummarizationRequest,
    ) -> Result<String, SummarizationClientError> {
        let payload = json!({
            "model": request.model,
            "prompt": request.prompt,
            "stream": false,
            "options": {
                // Lower temperature for deterministic summaries.
                "temperature": 0.1,
            }
        });

        let response = self
            .http
            .post(self.endpoint())
            .json(&payload)
            .send()
            .await
            .map_err(|error| {
                SummarizationClientError::ProviderUnavailable(format!(
                    "failed to reach Ollama at {}: {error}",
                    self.base_url
                ))
            })?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(SummarizationClientError::ProviderUnavailable(format!(
                "Ollama endpoint {} returned 404",
                self.endpoint()
            )));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(SummarizationClientError::GenerationFailed(format!(
                "Ollama returned {status}: {body}"
            )));
        }

        let body: OllamaResponse = response.json().await.map_err(|error| {
            SummarizationClientError::InvalidResponse(format!(
                "failed to decode Ollama response: {error}"
            ))
        })?;

        if !body.done {
            return Err(SummarizationClientError::InvalidResponse(
                "Ollama response incomplete (streaming not supported)".into(),
            ));
        }

        Ok(body.response.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::POST, MockServer};

    #[tokio::test]
    async fn ollama_client_handles_successful_response() {
        let server = MockServer::start_async().await;
        let client = OllamaSummarizationClient {
            http: Client::builder()
                .user_agent("rusty-mem-test")
                .build()
                .expect("client"),
            base_url: server.base_url(),
        };

        let mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/api/generate");
                then.status(200).json_body(json!({
                    "response": "Summary text",
                    "done": true
                }));
            })
            .await;

        let summary = client
            .generate_summary(SummarizationRequest {
                model: "llama".into(),
                prompt: "Summarize".into(),
                max_words: 100,
            })
            .await
            .expect("summary");

        mock.assert();
        assert_eq!(summary, "Summary text");
    }

    #[tokio::test]
    async fn ollama_client_handles_error_status() {
        let server = MockServer::start_async().await;
        let client = OllamaSummarizationClient {
            http: Client::builder()
                .user_agent("rusty-mem-test")
                .build()
                .expect("client"),
            base_url: server.base_url(),
        };

        server
            .mock_async(|when, then| {
                when.method(POST).path("/api/generate");
                then.status(500).body("boom");
            })
            .await;

        let error = client
            .generate_summary(SummarizationRequest {
                model: "llama".into(),
                prompt: "Summarize".into(),
                max_words: 100,
            })
            .await
            .expect_err("error response");

        matches!(error, SummarizationClientError::GenerationFailed(message) if message.contains("500"));
    }
}
