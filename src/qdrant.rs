use crate::config::get_config;
use reqwest::{Client, Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
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

/// Optional metadata fields propagated into each Qdrant payload.
#[derive(Debug, Clone, Default)]
pub struct PayloadOverrides {
    /// Override for the `project_id` field.
    pub project_id: Option<String>,
    /// Override for the `memory_type` field.
    pub memory_type: Option<String>,
    /// Tags attached to each chunk for filterable metadata.
    pub tags: Option<Vec<String>>,
    /// Optional URI describing the chunk source.
    pub source_uri: Option<String>,
}

/// Prepared point ready for indexing, including text, hash, and vector.
#[derive(Debug, Clone)]
pub struct PointInsert {
    /// Raw chunk text.
    pub text: String,
    /// Deterministic hash of the chunk used for dedupe.
    pub chunk_hash: String,
    /// Embedding vector produced for the chunk.
    pub vector: Vec<f32>,
}

/// Summary describing how Qdrant applied an indexing request.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexSummary {
    /// Number of new vectors inserted by the request.
    pub inserted: usize,
    /// Number of vectors updated in place.
    pub updated: usize,
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
        points: Vec<PointInsert>,
        overrides: &PayloadOverrides,
    ) -> Result<IndexSummary, QdrantError> {
        if points.is_empty() {
            return Ok(IndexSummary::default());
        }

        let now = current_timestamp_rfc3339();
        let serialized: Vec<_> = points
            .into_iter()
            .map(|point| {
                let memory_id = Uuid::new_v4().to_string();
                let payload =
                    build_payload(&memory_id, &point.text, &now, &point.chunk_hash, overrides);
                json!({
                    "id": memory_id,
                    "vector": point.vector,
                    "payload": payload,
                })
            })
            .collect();

        let point_count = serialized.len();
        let response = self
            .request(
                Method::PUT,
                &format!("collections/{}/points", collection_name),
            )?
            .query(&[("wait", true)])
            .json(&json!({ "points": serialized }))
            .send()
            .await?;

        self.ensure_success(response, || {
            tracing::debug!(
                collection = collection_name,
                points = point_count,
                "Points indexed"
            );
        })
        .await?;

        Ok(IndexSummary {
            inserted: point_count,
            updated: 0,
        })
    }

    /// Ensure standard payload indexes exist for common filters.
    pub async fn ensure_payload_indexes(&self, collection_name: &str) -> Result<(), QdrantError> {
        // Fields and their schemas to index.
        let fields: [(&str, &str); 5] = [
            ("project_id", "keyword"),
            ("memory_type", "keyword"),
            ("tags", "keyword"),
            ("timestamp", "datetime"),
            ("chunk_hash", "keyword"),
        ];

        for (field, schema) in fields {
            let body = json!({
                "field_name": field,
                "field_schema": schema,
            });

            let response = self
                .request(Method::PUT, &format!("collections/{collection_name}/index"))?
                .json(&body)
                .send()
                .await?;

            // 2xx is success; 409 conflict means already exists in some versions — treat as ok.
            if response.status().is_success() {
                tracing::debug!(
                    collection = collection_name,
                    field,
                    schema,
                    "Payload index ensured"
                );
            } else if response.status() == StatusCode::CONFLICT {
                tracing::debug!(
                    collection = collection_name,
                    field,
                    schema,
                    "Payload index already exists"
                );
            } else {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let error = QdrantError::UnexpectedStatus { status, body };
                tracing::warn!(collection = collection_name, field, schema, error = %error, "Failed to ensure payload index");
            }
        }

        Ok(())
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

fn build_payload(
    memory_id: &str,
    text: &str,
    timestamp_rfc3339: &str,
    chunk_hash: &str,
    overrides: &PayloadOverrides,
) -> Value {
    let mut payload = Map::new();
    payload.insert("memory_id".into(), Value::String(memory_id.to_string()));
    payload.insert(
        "project_id".into(),
        Value::String(
            overrides
                .project_id
                .clone()
                .unwrap_or_else(default_project_id),
        ),
    );
    payload.insert(
        "memory_type".into(),
        Value::String(
            overrides
                .memory_type
                .clone()
                .unwrap_or_else(default_memory_type),
        ),
    );
    payload.insert(
        "timestamp".into(),
        Value::String(timestamp_rfc3339.to_string()),
    );
    payload.insert("chunk_hash".into(), Value::String(chunk_hash.to_string()));
    payload.insert("text".into(), Value::String(text.to_string()));

    if let Some(source_uri) = overrides
        .source_uri
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        payload.insert("source_uri".into(), Value::String(source_uri.clone()));
    }

    if let Some(tags) = overrides.tags.as_ref().filter(|tags| !tags.is_empty()) {
        payload.insert(
            "tags".into(),
            Value::Array(tags.iter().map(|tag| Value::String(tag.clone())).collect()),
        );
    }

    Value::Object(payload)
}

/// Compute a deterministic SHA-256 hash for the chunk text.
pub fn compute_chunk_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

fn current_timestamp_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn default_project_id() -> String {
    "default".to_string()
}

fn default_memory_type() -> String {
    "semantic".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_hash_is_stable() {
        let text = "Hello world";
        let h1 = compute_chunk_hash(text);
        let h2 = compute_chunk_hash(text);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn timestamp_is_rfc3339_like() {
        let ts = current_timestamp_rfc3339();
        assert!(ts.contains('T') && ts.ends_with('Z'));
    }

    #[test]
    fn payload_includes_defaults_and_text() {
        let id = Uuid::new_v4().to_string();
        let now = "2025-01-01T00:00:00Z";
        let chunk_hash = "abc123";
        let payload = build_payload(&id, "sample", now, chunk_hash, &PayloadOverrides::default());
        assert_eq!(payload["memory_id"], id);
        assert_eq!(payload["project_id"], default_project_id());
        assert_eq!(payload["memory_type"], default_memory_type());
        assert_eq!(payload["timestamp"], now);
        assert_eq!(payload["text"], "sample");
        assert_eq!(payload["chunk_hash"], chunk_hash);
    }

    #[test]
    fn payload_applies_overrides() {
        let id = Uuid::new_v4().to_string();
        let now = "2025-01-01T00:00:00Z";
        let overrides = PayloadOverrides {
            project_id: Some("proj".into()),
            memory_type: Some("episodic".into()),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            source_uri: Some("file://doc".into()),
        };
        let payload = build_payload(&id, "sample", now, "hash", &overrides);
        assert_eq!(payload["project_id"], "proj");
        assert_eq!(payload["memory_type"], "episodic");
        assert_eq!(payload["source_uri"], "file://doc");
        let tags = payload["tags"].as_array().expect("tags present");
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().any(|tag| tag == "alpha"));
        assert!(tags.iter().any(|tag| tag == "beta"));
    }
}
