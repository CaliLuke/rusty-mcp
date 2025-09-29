//! HTTP client wrapper for interacting with Qdrant.

use crate::config::get_config;
use crate::qdrant::types::PayloadOverrides;
use crate::qdrant::{
    filters::{accumulate_project_id, accumulate_tags},
    payload::{build_payload, current_timestamp_rfc3339, generate_memory_id},
    types::{
        IndexSummary, ListCollectionsResponse, QdrantError, QueryResponse, QueryResponseResult,
        ScoredPoint, ScrollResponse,
    },
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Lightweight HTTP client for Qdrant operations.
pub struct QdrantService {
    pub(crate) client: Client,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
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

    /// Enumerate distinct project identifiers stored in the collection payloads.
    pub async fn list_projects(&self, collection: &str) -> Result<BTreeSet<String>, QdrantError> {
        let payloads = self
            .scroll_payloads(collection, json!(["project_id"]), None)
            .await?;
        let mut projects = BTreeSet::new();
        for payload in payloads {
            accumulate_project_id(&payload, &mut projects);
        }
        Ok(projects)
    }

    /// Enumerate distinct tags stored in the collection payloads, optionally scoped by project.
    pub async fn list_tags(
        &self,
        collection: &str,
        project_id: Option<&str>,
    ) -> Result<BTreeSet<String>, QdrantError> {
        let filter = project_id.map(|project| {
            json!({
                "must": [
                    {
                        "key": "project_id",
                        "match": { "value": project }
                    }
                ]
            })
        });

        let payloads = self
            .scroll_payloads(collection, json!(["tags"]), filter)
            .await?;
        let mut tags = BTreeSet::new();
        for payload in payloads {
            accumulate_tags(&payload, &mut tags);
        }
        Ok(tags)
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
        points: Vec<crate::qdrant::types::PointInsert>,
        overrides: &PayloadOverrides,
    ) -> Result<IndexSummary, QdrantError> {
        if points.is_empty() {
            return Ok(IndexSummary::default());
        }

        let now = current_timestamp_rfc3339();
        let serialized: Vec<_> = points
            .into_iter()
            .map(|point| {
                let memory_id = generate_memory_id();
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

    /// Perform a similarity search against a collection, returning scored payloads.
    pub async fn search_points(
        &self,
        collection_name: &str,
        vector: Vec<f32>,
        filter: Option<Value>,
        limit: usize,
        score_threshold: Option<f32>,
        using: Option<String>,
    ) -> Result<Vec<ScoredPoint>, QdrantError> {
        let mut body = json!({
            "query": vector,
            "limit": limit,
            "with_payload": true,
        });
        let obj = body
            .as_object_mut()
            .expect("query body should remain an object");

        if let Some(name) = using.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }) {
            obj.insert("using".into(), Value::String(name));
        }

        if let Some(threshold) = score_threshold {
            obj.insert("score_threshold".into(), Value::from(threshold));
        }

        if let Some(filter_value) = filter {
            obj.insert("filter".into(), filter_value);
        }

        let response = self
            .request(
                Method::POST,
                &format!("collections/{collection_name}/points/query"),
            )?
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let error = QdrantError::UnexpectedStatus { status, body };
            tracing::error!(collection = collection_name, error = %error, "Qdrant search failed");
            return Err(error);
        }

        let payload: QueryResponse = response.json().await?;
        let points = match payload.result {
            QueryResponseResult::Points(points) => points,
            QueryResponseResult::Object { points, .. } => points,
        };
        let results = points
            .into_iter()
            .map(|point| ScoredPoint {
                id: stringify_point_id(point.id),
                score: point.score,
                payload: point.payload,
            })
            .collect();

        Ok(results)
    }

    /// Ensure standard payload indexes exist for common filters.
    pub async fn ensure_payload_indexes(&self, collection_name: &str) -> Result<(), QdrantError> {
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

    async fn scroll_payloads(
        &self,
        collection: &str,
        with_payload: Value,
        filter: Option<Value>,
    ) -> Result<Vec<Map<String, Value>>, QdrantError> {
        let mut offset: Option<Value> = None;
        let mut payloads = Vec::new();
        let filter_body = filter.unwrap_or_else(|| json!({ "must": [] }));

        loop {
            let mut body = json!({
                "with_payload": with_payload.clone(),
                "with_vector": false,
                "limit": 512,
                "offset": offset.clone().unwrap_or(Value::Null),
                "filter": filter_body.clone(),
            });

            if offset.is_none() {
                body.as_object_mut().unwrap().remove("offset");
            }

            let response = self
                .request(
                    Method::POST,
                    &format!("collections/{collection}/points/scroll"),
                )?
                .json(&body)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let error = QdrantError::UnexpectedStatus { status, body };
                tracing::error!(collection, error = %error, "Failed to scroll payloads");
                return Err(error);
            }

            let ScrollResponse { result } = response.json().await?;
            for point in result.points {
                if let Some(payload) = point.payload {
                    payloads.push(payload);
                }
            }

            match result.next_page_offset {
                Some(next) => offset = Some(next),
                None => break,
            }
        }

        Ok(payloads)
    }

    /// Scroll payloads and return their associated point identifiers.
    pub async fn scroll_payloads_with_ids(
        &self,
        collection: &str,
        with_payload: Value,
        filter: Option<Value>,
    ) -> Result<Vec<(String, Map<String, Value>)>, QdrantError> {
        let mut offset: Option<Value> = None;
        let mut results = Vec::new();
        let filter_body = filter.unwrap_or_else(|| json!({ "must": [] }));

        loop {
            let body = json!({
                "with_payload": with_payload.clone(),
                "with_vector": false,
                "limit": 512,
                "offset": offset.clone().unwrap_or(Value::Null),
                "filter": filter_body,
                "order_by": [
                    { "key": "timestamp", "direction": "asc" }
                ]
            });

            // Qdrant does not yet support `order_by` in scroll for all versions; keep it in body but tolerate errors.
            let response = self
                .request(
                    Method::POST,
                    &format!("collections/{collection}/points/scroll"),
                )?
                .json(&body)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let error = QdrantError::UnexpectedStatus { status, body };
                tracing::error!(collection, error = %error, "Failed to scroll payloads with ids");
                return Err(error);
            }

            let ScrollResponse { result } = response.json().await?;
            for point in result.points {
                if let (Some(id), Some(payload)) = (point.id, point.payload) {
                    results.push((stringify_point_id(id), payload));
                }
            }

            match result.next_page_offset {
                Some(next) => offset = Some(next),
                None => break,
            }
        }

        Ok(results)
    }
}

fn normalize_base_url(url: &str) -> Result<String, String> {
    let mut parsed = reqwest::Url::parse(url).map_err(|err| err.to_string())?;
    let path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(&path);
    Ok(parsed.to_string())
}

fn format_endpoint(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn stringify_point_id(id: Value) -> String {
    match id {
        Value::String(text) => text,
        Value::Number(number) => number.to_string(),
        Value::Object(map) => map
            .get("uuid")
            .map(|value| match value {
                Value::String(uuid) => uuid.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| Value::Object(map).to_string()),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::POST, MockServer};
    use reqwest::Client;

    #[tokio::test]
    async fn search_points_emits_expected_request() {
        let server = MockServer::start_async().await;

        let filter = crate::qdrant::build_search_filter(&crate::qdrant::SearchFilterArgs {
            project_id: Some("repo-a".into()),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            ..Default::default()
        })
        .expect("filter value");

        let mock = server
            .mock_async(|when, then| {
                when.method(POST).path("/collections/demo/points/query");
                then.status(200).json_body(json!({
                    "status": "ok",
                    "time": 0.0,
                    "result": [
                        {
                            "id": "memory-1",
                            "score": 0.42,
                            "payload": {
                                "text": "Example",
                                "project_id": "repo-a"
                            }
                        }
                    ]
                }));
            })
            .await;

        let service = QdrantService {
            client: Client::builder()
                .user_agent("rusty-mem-test")
                .build()
                .expect("client"),
            base_url: server.base_url(),
            api_key: None,
        };

        let results = service
            .search_points(
                "demo",
                vec![0.1, 0.2],
                Some(filter.clone()),
                3,
                Some(0.25),
                None,
            )
            .await
            .expect("search request");

        mock.assert();

        assert_eq!(results.len(), 1);
        let hit = &results[0];
        assert_eq!(hit.id, "memory-1");
        assert!((hit.score - 0.42).abs() < f32::EPSILON);
        let payload = hit.payload.as_ref().expect("payload");
        assert_eq!(payload["project_id"], Value::String("repo-a".into()));
        assert_eq!(payload["text"], Value::String("Example".into()));
    }
}
