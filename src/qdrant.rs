use crate::config::get_config;
use reqwest::{Client, Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

/// Errors returned while interacting with Qdrant.
#[derive(Debug, Error)]
pub enum QdrantError {
    /// Base URL failed to parse or normalize.
    #[error("Invalid Qdrant URL: {0}")]
    InvalidUrl(String),
    /// HTTP layer failed before receiving a response.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Qdrant responded with an unexpected status code.
    #[error("Unexpected Qdrant response ({status}): {body}")]
    UnexpectedStatus {
        /// HTTP status returned from Qdrant.
        status: StatusCode,
        /// Body payload associated with the failing response.
        body: String,
    },
}

/// Lightweight HTTP client for Qdrant operations.
pub struct QdrantService {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl QdrantService {
    /// Construct a new client using configuration derived from the environment.
    pub fn new() -> Result<Self, QdrantError> {
        let config = get_config();
        let client = Client::builder().user_agent("rusty-mem/0.1").build()?;

        let base_url = normalize_base_url(&config.qdrant_url).map_err(QdrantError::InvalidUrl)?;
        tracing::debug!(
            url = %base_url,
            has_api_key = %config
                .qdrant_api_key
                .as_deref()
                .map(|value| !value.is_empty())
                .unwrap_or(false),
            "Initialized Qdrant HTTP client"
        );

        Ok(Self {
            client,
            base_url,
            api_key: config.qdrant_api_key.clone(),
        })
    }

    /// Create a collection only when it is missing from Qdrant.
    pub async fn create_collection_if_not_exists(
        &self,
        collection_name: &str,
        vector_size: u64,
    ) -> Result<(), QdrantError> {
        if self.collection_exists(collection_name).await? {
            return Ok(());
        }

        tracing::debug!(
            collection = collection_name,
            vector_size,
            "Creating collection"
        );
        self.create_collection(collection_name, vector_size).await
    }

    /// Create or update a collection with the specified vector size.
    pub async fn create_collection(
        &self,
        collection_name: &str,
        vector_size: u64,
    ) -> Result<(), QdrantError> {
        let body = json!({
            "vectors": {
                "size": vector_size,
                "distance": "Cosine"
            }
        });

        let response = self
            .request(Method::PUT, &format!("collections/{collection_name}"))?
            .json(&body)
            .send()
            .await?;

        self.ensure_success(response, || {
            tracing::debug!(collection = collection_name, "Collection ensured/created");
        })
        .await
    }

    /// Retrieve the names of all collections present in Qdrant.
    pub async fn list_collections(&self) -> Result<Vec<String>, QdrantError> {
        let response = self.request(Method::GET, "collections")?.send().await?;

        if response.status().is_success() {
            let payload: ListCollectionsResponse = response.json().await?;
            let names = payload
                .result
                .collections
                .into_iter()
                .map(|collection| collection.name)
                .collect();
            Ok(names)
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let error = QdrantError::UnexpectedStatus { status, body };
            tracing::error!(error = %error, "Failed to list collections");
            Err(error)
        }
    }

    /// Upload new vectors to the given collection.
    pub async fn index_points(
        &self,
        collection_name: &str,
        texts: Vec<String>,
        vectors: Vec<Vec<f32>>,
    ) -> Result<(), QdrantError> {
        let points: Vec<_> = texts
            .into_iter()
            .zip(vectors.into_iter())
            .map(|(text, vector)| {
                json!({
                    "id": Uuid::new_v4().to_string(),
                    "vector": vector,
                    "payload": { "text": text }
                })
            })
            .collect();

        let point_count = points.len();
        let response = self
            .request(
                Method::PUT,
                &format!("collections/{}/points", collection_name),
            )?
            .query(&[("wait", true)])
            .json(&json!({ "points": points }))
            .send()
            .await?;

        self.ensure_success(response, || {
            tracing::debug!(
                collection = collection_name,
                points = point_count,
                "Points indexed"
            );
        })
        .await
    }

    async fn collection_exists(&self, collection_name: &str) -> Result<bool, QdrantError> {
        let response = self
            .request(Method::GET, &format!("collections/{collection_name}"))?
            .send()
            .await?;

        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => {
                let body = response.text().await.unwrap_or_default();
                let error = QdrantError::UnexpectedStatus { status, body };
                tracing::error!(collection = collection_name, error = %error, "Collection existence check failed");
                Err(error)
            }
        }
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, QdrantError> {
        let url = format_endpoint(&self.base_url, path);
        let mut req = self.client.request(method, url);
        if let Some(api_key) = &self.api_key
            && !api_key.is_empty()
        {
            req = req.header("api-key", api_key);
        }
        Ok(req)
    }

    async fn ensure_success<F>(
        &self,
        response: reqwest::Response,
        on_success: F,
    ) -> Result<(), QdrantError>
    where
        F: FnOnce(),
    {
        if response.status().is_success() {
            on_success();
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let error = QdrantError::UnexpectedStatus { status, body };
            tracing::error!(error = %error, "Qdrant request failed");
            Err(error)
        }
    }
}

fn normalize_base_url(url: &str) -> Result<String, String> {
    let mut parsed = Url::parse(url).map_err(|err| err.to_string())?;
    let path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(&path);
    Ok(parsed.to_string())
}

fn format_endpoint(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

#[derive(Deserialize)]
struct ListCollectionsResponse {
    result: ListCollectionsResult,
}

#[derive(Deserialize)]
struct ListCollectionsResult {
    collections: Vec<CollectionDescription>,
}

#[derive(Deserialize)]
struct CollectionDescription {
    name: String,
}
