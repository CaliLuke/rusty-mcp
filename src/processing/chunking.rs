//! Chunk-size heuristics and semantic chunking helpers.
//!
//! This module encapsulates how Rusty Memory determines chunk boundaries and token budgets.
//! Highlights:
//!
//! - Automatic sizing: derive a budget from the embedding model’s context window and clamp to
//!   a conservative range; callers can override via `TEXT_SPLITTER_CHUNK_SIZE`.
//! - Safe defaults: opt into smaller chunks (`window/8`) using
//!   `TEXT_SPLITTER_USE_SAFE_DEFAULTS=1` to bias toward retrieval precision.
//! - Overlap: optionally include a sliding token overlap (`TEXT_SPLITTER_CHUNK_OVERLAP`) so that
//!   spans around boundaries remain visible to retrieval and downstream prompts.
//! - Token counting: prefer `tiktoken-rs` for OpenAI/known encodings; fall back to a whitespace
//!   counter when the model’s tokenizer is unavailable (common for some Ollama models).

use crate::config::EmbeddingProvider;
use anyhow::Error as TokenizerError;
use semchunk_rs::Chunker;
use std::sync::Arc;
use tiktoken_rs::{
    CoreBPE, cl100k_base, get_bpe_from_model, model::get_context_size, o200k_base, p50k_base,
    p50k_edit, r50k_base,
};

use super::types::ChunkingError;

type TokenCounter = Arc<dyn Fn(&str) -> usize + Send + Sync>;

const MIN_AUTOMATIC_CHUNK_SIZE: usize = 256;
const MAX_AUTOMATIC_CHUNK_SIZE: usize = 1024;

/// Determine the chunk size for a request, respecting overrides and safe defaults.
///
/// Precedence:
/// 1) Explicit override (e.g., `TEXT_SPLITTER_CHUNK_SIZE`) wins and is clamped at `>= 1`.
/// 2) Otherwise, derive from the provider/model context window and divide by `4` (or `8` when
///    `use_safe_defaults` is true). The result is clamped into `[256, 1024]`.
///
/// The derived size is logged by the processing service and exposed via metrics (`lastChunkSize`).
pub(crate) fn determine_chunk_size(
    override_size: Option<usize>,
    provider: EmbeddingProvider,
    model: &str,
    use_safe_defaults: bool,
) -> usize {
    if let Some(explicit) = override_size {
        return explicit.max(1);
    }

    let window = embedding_context_window(provider, model);
    let divisor = if use_safe_defaults { 8 } else { 4 };
    let base = (window / divisor).max(1);
    let candidate = base.max(MIN_AUTOMATIC_CHUNK_SIZE);
    candidate.clamp(MIN_AUTOMATIC_CHUNK_SIZE, MAX_AUTOMATIC_CHUNK_SIZE)
}

/// Look up the embedding context window for a given provider/model combination.
pub(crate) fn embedding_context_window(provider: EmbeddingProvider, model: &str) -> usize {
    match provider {
        EmbeddingProvider::OpenAI => openai_embedding_context_window(model),
        EmbeddingProvider::Ollama => ollama_embedding_context_window(model),
    }
}

fn openai_embedding_context_window(model: &str) -> usize {
    if model.starts_with("text-embedding-3") {
        return 8192;
    }
    if model.starts_with("text-embedding-ada-002") {
        return 8192;
    }

    let size = get_context_size(model);
    if size == 4096 && model.contains("embedding") {
        tracing::debug!(model, "Using default embedding context window fallback");
    }
    size
}

fn ollama_embedding_context_window(model: &str) -> usize {
    let normalized = model.to_lowercase();
    match normalized.as_str() {
        "nomic-embed-text" | "mxbai-embed-large" | "mxbai-embed-large-v1" => 8192,
        value if value.contains("all-minilm") => 512,
        value if value.contains("e5-large") => 4096,
        _ => {
            tracing::trace!(model, "Using default Ollama context window estimate");
            4096
        }
    }
}

/// Chunk text into semantic segments using the configured token counter.
///
/// - `chunk_size` is a hard upper bound on the token count per segment.
/// - `overlap` requests a sliding-window overlap (tokens) between adjacent chunks after semantic
///   splitting; the function guarantees the final strings respect the token budget.
/// - Tokenization uses `tiktoken` when possible and falls back to whitespace counting.
///
/// Returns an empty vector when the input text is all whitespace.
pub(crate) fn chunk_text(
    text: &str,
    chunk_size: usize,
    overlap: usize,
    provider: EmbeddingProvider,
    model: &str,
) -> Result<Vec<String>, ChunkingError> {
    if chunk_size == 0 {
        return Err(ChunkingError::InvalidChunkSize);
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }

    let token_counter = build_token_counter(provider, model)?;
    Ok(chunk_text_with_counter(
        text,
        chunk_size,
        overlap,
        token_counter,
    ))
}

/// Build a token counter for the given provider/model.
///
/// Uses OpenAI encodings when possible and gracefully falls back to whitespace tokenization for
/// unknown or locally aliased models (typical with Ollama). The fallback is logged at `warn` level
/// to aid diagnosis while keeping ingestion flowing.
pub(crate) fn build_token_counter(
    provider: EmbeddingProvider,
    model: &str,
) -> Result<TokenCounter, ChunkingError> {
    match provider {
        EmbeddingProvider::OpenAI => build_tiktoken_counter(model),
        EmbeddingProvider::Ollama => match build_tiktoken_counter(model) {
            Ok(counter) => Ok(counter),
            Err(error) => {
                tracing::warn!(
                    model,
                    error = %error,
                    "Tokenizer unavailable for Ollama model; falling back to whitespace counter"
                );
                Ok(default_token_counter())
            }
        },
    }
}

fn build_tiktoken_counter(model: &str) -> Result<TokenCounter, ChunkingError> {
    let normalized = model.trim();
    let target = if normalized.is_empty() {
        "cl100k_base"
    } else {
        normalized
    };
    let encoding = resolve_encoding(target).map_err(|source| ChunkingError::Tokenizer {
        model: target.to_string(),
        source,
    })?;
    let encoding = Arc::new(encoding);

    Ok(Arc::new(move |segment: &str| {
        encoding.encode_ordinary(segment).len()
    }))
}

fn resolve_encoding(model: &str) -> Result<CoreBPE, TokenizerError> {
    match get_bpe_from_model(model) {
        Ok(encoding) => Ok(encoding),
        Err(model_err) => {
            tracing::debug!(
                model,
                error = %model_err,
                "Tokenizer model lookup failed; trying encoding name"
            );
            if let Some(candidate) = encoding_from_name(model) {
                candidate
            } else {
                tracing::warn!(
                    model,
                    "Falling back to 'cl100k_base' encoding for token counting"
                );
                cl100k_base()
            }
        }
    }
}

fn encoding_from_name(name: &str) -> Option<Result<CoreBPE, TokenizerError>> {
    match name {
        "cl100k_base" => Some(cl100k_base()),
        "o200k_base" => Some(o200k_base()),
        "p50k_base" => Some(p50k_base()),
        "p50k_edit" => Some(p50k_edit()),
        "r50k_base" | "gpt2" => Some(r50k_base()),
        _ => None,
    }
}

fn default_token_counter() -> TokenCounter {
    Arc::new(|segment: &str| {
        let tokens = segment.split_whitespace().count();
        if tokens == 0 && !segment.is_empty() {
            1
        } else {
            tokens
        }
    })
}

/// Lower-level chunker that accepts an explicit token counter.
///
/// You likely want [`chunk_text`]; this helper exists for tests and for callers that need to
/// plug in a custom token counter.
fn chunk_text_with_counter(
    text: &str,
    chunk_size: usize,
    overlap: usize,
    token_counter: TokenCounter,
) -> Vec<String> {
    let counter_for_chunker = token_counter.clone();
    let chunker = Chunker::new(
        chunk_size,
        Box::new(move |segment: &str| counter_for_chunker.as_ref()(segment)),
    );
    let base_chunks = chunker.chunk(text);
    apply_overlap(base_chunks, chunk_size, overlap, &token_counter)
}

/// Apply a token-limited overlap between the tail of the previous chunk and the current one.
///
/// Ensures the resulting overlapped chunk does not exceed `chunk_size` by trimming from the
/// start as needed.
fn apply_overlap(
    chunks: Vec<String>,
    chunk_size: usize,
    overlap: usize,
    token_counter: &TokenCounter,
) -> Vec<String> {
    if chunks.is_empty() {
        return chunks;
    }

    let effective_overlap = overlap.min(chunk_size.saturating_sub(1));
    if effective_overlap == 0 {
        return chunks;
    }

    let mut overlapped = Vec::with_capacity(chunks.len());
    let mut iter = chunks.into_iter();
    let mut previous = iter
        .next()
        .expect("chunks iterator yielded zero elements despite non-empty guard");
    overlapped.push(previous.clone());

    for current in iter {
        let overlapped_chunk = build_overlapped_chunk(
            &previous,
            &current,
            effective_overlap,
            chunk_size,
            token_counter,
        );
        overlapped.push(overlapped_chunk);
        previous = current;
    }

    overlapped
}

fn build_overlapped_chunk(
    previous: &str,
    current: &str,
    overlap: usize,
    chunk_size: usize,
    token_counter: &TokenCounter,
) -> String {
    if overlap == 0 {
        return current.to_string();
    }

    let tail = tail_with_token_limit(previous, overlap, token_counter);
    let mut combined = String::with_capacity(tail.len() + current.len() + 1);

    if !tail.is_empty() {
        combined.push_str(tail);
        if !ends_with_whitespace(tail) && !starts_with_whitespace(current) {
            combined.push(' ');
        }
    }

    combined.push_str(current);
    trim_to_token_budget(&combined, chunk_size, token_counter)
}

fn tail_with_token_limit<'a>(
    text: &'a str,
    token_limit: usize,
    token_counter: &TokenCounter,
) -> &'a str {
    if token_limit == 0 {
        return "";
    }

    let trimmed_text = text.trim_start();
    if token_counter.as_ref()(trimmed_text) <= token_limit {
        return trimmed_text;
    }

    let len = text.len();
    let mut start = 0;

    while start < len {
        let next_start = text[start..]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset)
            .unwrap_or(len);
        start = next_start;
        let candidate = &text[start..];
        let trimmed = candidate.trim_start();
        if token_counter.as_ref()(trimmed) <= token_limit {
            return trimmed;
        }
    }

    ""
}

fn trim_to_token_budget(text: &str, token_budget: usize, token_counter: &TokenCounter) -> String {
    if token_budget == 0 {
        return String::new();
    }

    if token_counter.as_ref()(text) <= token_budget {
        return text.to_string();
    }

    let len = text.len();
    let mut start = 0;

    while start < len {
        let next_start = text[start..]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset)
            .unwrap_or(len);
        start = next_start;
        let candidate = &text[start..];
        let trimmed = candidate.trim_start();
        if token_counter.as_ref()(trimmed) <= token_budget {
            return trimmed.to_string();
        }
    }

    String::new()
}

fn starts_with_whitespace(text: &str) -> bool {
    text.chars()
        .next()
        .map(|c| c.is_whitespace())
        .unwrap_or(false)
}

fn ends_with_whitespace(text: &str) -> bool {
    text.chars()
        .next_back()
        .map(|c| c.is_whitespace())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_respects_chunk_size_whitespace_counter() {
        let text = "one two three four five";
        let chunks = chunk_text_with_counter(text, 2, 0, default_token_counter());
        assert_eq!(chunks, vec!["one two", "three four", "five"]);
    }

    #[test]
    fn chunk_text_handles_empty_input() {
        let chunks = chunk_text_with_counter("", 4, 0, default_token_counter());
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_text_with_counter_applies_overlap() {
        let text = "one two three four five";
        let counter = default_token_counter();
        let chunks = chunk_text_with_counter(text, 3, 1, counter.clone());
        assert_eq!(chunks, vec!["one two three", "three four five"]);
        for chunk in &chunks {
            assert!(counter.as_ref()(chunk) <= 3);
        }
    }

    #[test]
    fn chunk_text_rejects_zero_chunk_size() {
        let error = chunk_text(
            "hello",
            0,
            0,
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
        )
        .unwrap_err();
        assert!(matches!(error, ChunkingError::InvalidChunkSize));
    }

    #[test]
    fn chunk_text_uses_tiktoken_budget() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let chunks = chunk_text(
            text,
            5,
            0,
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
        )
        .expect("chunking succeeded");
        let token_counter = build_tiktoken_counter("text-embedding-3-small").unwrap();
        for chunk in &chunks {
            assert!(token_counter.as_ref()(chunk) <= 5);
        }
        let chunk_words: Vec<String> = chunks
            .iter()
            .flat_map(|chunk| chunk.split_whitespace().map(|word| word.to_string()))
            .collect();
        let original_words: Vec<String> = text
            .split_whitespace()
            .map(|word| word.to_string())
            .collect();
        assert_eq!(chunk_words, original_words);
    }

    #[test]
    fn determine_chunk_size_prefers_override() {
        let chunk_size = determine_chunk_size(
            Some(42),
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
            false,
        );
        assert_eq!(chunk_size, 42);
    }

    #[test]
    fn determine_chunk_size_infers_openai_embedding_window() {
        let chunk_size = determine_chunk_size(
            None,
            EmbeddingProvider::OpenAI,
            "text-embedding-3-small",
            false,
        );
        assert_eq!(chunk_size, 1024);
    }

    #[test]
    fn determine_chunk_size_handles_common_ollama_models() {
        let chunk_size =
            determine_chunk_size(None, EmbeddingProvider::Ollama, "nomic-embed-text", false);
        assert_eq!(chunk_size, 1024);

        let mini_chunk =
            determine_chunk_size(None, EmbeddingProvider::Ollama, "all-minilm-l6-v2", false);
        assert_eq!(mini_chunk, 256);
    }

    #[test]
    fn determine_chunk_size_safe_defaults_reduce_window_proportion() {
        let conservative =
            determine_chunk_size(None, EmbeddingProvider::Ollama, "custom-model", true);
        let aggressive =
            determine_chunk_size(None, EmbeddingProvider::Ollama, "custom-model", false);

        assert_eq!(aggressive, 1024);
        assert_eq!(conservative, 512);
    }
}
