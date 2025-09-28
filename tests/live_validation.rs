use std::{env, sync::Once};

use rustymcp::{config, embedding, processing::ProcessingService};

static INIT: Once = Once::new();

fn set_default_env(key: &str, value: &str) {
    let needs_value = env::var(key).map(|v| v.trim().is_empty()).unwrap_or(true);
    if needs_value {
        // SAFETY: Tests run serially via Once and we intentionally mutate process env.
        unsafe {
            env::set_var(key, value);
        }
    }
}

fn init_config_once() {
    INIT.call_once(|| {
        set_default_env("QDRANT_URL", "http://127.0.0.1:6333");
        set_default_env("QDRANT_COLLECTION_NAME", "rusty-mem");
        set_default_env("EMBEDDING_PROVIDER", "ollama");
        set_default_env("EMBEDDING_MODEL", "nomic-embed-text");
        set_default_env("EMBEDDING_DIMENSION", "768");
        set_default_env("OLLAMA_URL", "http://127.0.0.1:11434");
        config::init_config();
    });
}

#[tokio::test]
#[ignore = "Requires live Qdrant"]
async fn live_qdrant_health_snapshot() {
    init_config_once();
    let service = ProcessingService::new().await;
    let snapshot = service.qdrant_health().await;
    assert!(
        snapshot.reachable,
        "Qdrant should be reachable: {snapshot:?}"
    );
    assert!(
        snapshot.default_collection_present,
        "default collection must exist: {snapshot:?}"
    );
}

#[tokio::test]
#[ignore = "Requires live Ollama embeddings"]
async fn live_ollama_embedding_roundtrip() {
    init_config_once();
    let client = embedding::get_embedding_client();
    let vectors = client
        .generate_embeddings(vec!["rusty-mem live embedding".to_string()])
        .await
        .expect("failed to request embeddings from provider");
    assert_eq!(vectors.len(), 1, "expected embedding per input chunk");
    let dimension = config::get_config().embedding_dimension;
    assert_eq!(vectors[0].len(), dimension, "embedding dimension mismatch");
}
