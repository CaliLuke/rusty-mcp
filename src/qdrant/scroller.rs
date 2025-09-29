//! Streaming helpers for iterating Qdrant scroll endpoints without manual loops.

use async_stream::try_stream;
use futures_core::Stream;
use reqwest::Method;
use serde_json::{Map, Value, json};

use super::client::QdrantService;
use super::client::stringify_point_id;
use super::types::{QdrantError, ScrollResponse};

const DEFAULT_SCROLL_LIMIT: usize = 512;

/// Stream Qdrant payloads for a collection using the scroll API.
pub fn stream_payloads<'a>(
    service: &'a QdrantService,
    collection: &'a str,
    with_payload: Value,
    filter: Option<Value>,
) -> impl Stream<Item = Result<Map<String, Value>, QdrantError>> + 'a {
    try_stream! {
        let mut offset: Option<Value> = None;
        let payload_template = with_payload;
        let filter_body = filter.unwrap_or_else(|| json!({ "must": [] }));

        loop {
            let mut body = json!({
                "with_payload": payload_template.clone(),
                "with_vector": false,
                "limit": DEFAULT_SCROLL_LIMIT,
                "filter": filter_body.clone(),
                "order_by": [
                    { "key": "timestamp", "direction": "asc" }
                ],
            });

            body.as_object_mut()
                .expect("scroll body is object")
                .insert("offset".into(), offset.clone().unwrap_or(Value::Null));

            let mut request = service.client.request(
                Method::POST,
                format_endpoint(&service.base_url, &format!("collections/{collection}/points/scroll")),
            );

            if let Some(api_key) = &service.api_key && !api_key.is_empty() {
                request = request.header("api-key", api_key);
            }

            let response = request.json(&body).send().await?;

            let status = response.status();
            if status.is_success() {
                let ScrollResponse { result } = response.json().await?;
                for point in result.points {
                    if let Some(payload) = point.payload {
                        yield payload;
                    }
                }

                match result.next_page_offset {
                    Some(next) => offset = Some(next),
                    None => break,
                }
            } else {
                let body = response.text().await.unwrap_or_default();
                tracing::error!(collection = collection, status = %status, "Failed to scroll payloads via stream");
                Err(QdrantError::UnexpectedStatus { status, body })?;
            }
        }
    }
}

/// Stream Qdrant payloads along with their point identifiers.
pub fn stream_payloads_with_ids<'a>(
    service: &'a QdrantService,
    collection: &'a str,
    with_payload: Value,
    filter: Option<Value>,
) -> impl Stream<Item = Result<(String, Map<String, Value>), QdrantError>> + 'a {
    try_stream! {
        let mut offset: Option<Value> = None;
        let payload_template = with_payload;
        let filter_body = filter.unwrap_or_else(|| json!({ "must": [] }));

        loop {
            let mut body = json!({
                "with_payload": payload_template.clone(),
                "with_vector": false,
                "limit": DEFAULT_SCROLL_LIMIT,
                "filter": filter_body.clone(),
                "order_by": [
                    { "key": "timestamp", "direction": "asc" }
                ],
            });

            body.as_object_mut()
                .expect("scroll body is object")
                .insert("offset".into(), offset.clone().unwrap_or(Value::Null));

            let mut request = service.client.request(
                Method::POST,
                format_endpoint(&service.base_url, &format!("collections/{collection}/points/scroll")),
            );

            if let Some(api_key) = &service.api_key && !api_key.is_empty() {
                request = request.header("api-key", api_key);
            }

            let response = request.json(&body).send().await?;

            let status = response.status();
            if status.is_success() {
                let ScrollResponse { result } = response.json().await?;
                for point in result.points {
                    if let (Some(id), Some(payload)) = (point.id, point.payload) {
                        yield (stringify_point_id(id), payload);
                    }
                }

                match result.next_page_offset {
                    Some(next) => offset = Some(next),
                    None => break,
                }
            } else {
                let body = response.text().await.unwrap_or_default();
                tracing::error!(collection = collection, status = %status, "Failed to scroll payloads with ids via stream");
                Err(QdrantError::UnexpectedStatus { status, body })?;
            }
        }
    }
}

fn format_endpoint(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{pin_mut, stream::StreamExt};
    use httpmock::{Method::POST, MockServer};

    #[tokio::test]
    async fn stream_payloads_collects_multiple_pages() {
        let server = MockServer::start_async().await;
        let service = QdrantService {
            client: reqwest::Client::builder()
                .user_agent("rusty-mem-test")
                .build()
                .expect("client"),
            base_url: server.base_url(),
            api_key: None,
        };

        let first = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/collections/demo/points/scroll")
                    .body_contains("\"offset\":null");
                then.status(200).json_body(json!({
                    "result": {
                        "points": [
                            { "payload": { "value": 1 } }
                        ],
                        "next_page_offset": { "offset": 1 }
                    }
                }));
            })
            .await;

        let second = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/collections/demo/points/scroll")
                    .body_contains("\"offset\":{\"offset\":1}");
                then.status(200).json_body(json!({
                    "result": {
                        "points": [
                            { "payload": { "value": 2 } }
                        ],
                        "next_page_offset": null
                    }
                }));
            })
            .await;

        let stream = stream_payloads(&service, "demo", json!(["value"]), None);
        pin_mut!(stream);
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item.expect("payload"));
        }

        first.assert();
        second.assert();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].get("value").and_then(Value::as_i64), Some(1));
        assert_eq!(items[1].get("value").and_then(Value::as_i64), Some(2));
    }

    #[tokio::test]
    async fn stream_payloads_with_ids_collects_multiple_pages() {
        let server = MockServer::start_async().await;
        let service = QdrantService {
            client: reqwest::Client::builder()
                .user_agent("rusty-mem-test")
                .build()
                .expect("client"),
            base_url: server.base_url(),
            api_key: None,
        };

        let first_page = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/collections/demo/points/scroll")
                    .body_contains("\"offset\":null");
                then.status(200).json_body(json!({
                    "result": {
                        "points": [
                            { "id": "a", "payload": { "value": 10 } }
                        ],
                        "next_page_offset": { "offset": 2 }
                    }
                }));
            })
            .await;

        let second_page = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/collections/demo/points/scroll")
                    .body_contains("\"offset\":{\"offset\":2}");
                then.status(200).json_body(json!({
                    "result": {
                        "points": [
                            { "id": "b", "payload": { "value": 20 } }
                        ],
                        "next_page_offset": null
                    }
                }));
            })
            .await;

        let stream = stream_payloads_with_ids(&service, "demo", json!(["value"]), None);
        pin_mut!(stream);
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item.expect("entry"));
        }

        first_page.assert();
        second_page.assert();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "a");
        assert_eq!(items[1].0, "b");
    }
}
