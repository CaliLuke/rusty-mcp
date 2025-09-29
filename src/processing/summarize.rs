//! Helper routines for the summarization pipeline.

use crate::processing::types::SearchTimeRange;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Episodic memory loaded for summarization.
#[derive(Debug, Clone)]
pub(crate) struct EpisodicMemory {
    pub(crate) memory_id: String,
    pub(crate) text: String,
    pub(crate) timestamp: Option<String>,
    pub(crate) parsed_timestamp: Option<OffsetDateTime>,
}

impl EpisodicMemory {
    pub(crate) fn new(memory_id: String, text: String, timestamp: Option<String>) -> Self {
        let parsed_timestamp = timestamp
            .as_ref()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
        Self {
            memory_id,
            text,
            timestamp,
            parsed_timestamp,
        }
    }
}

/// Sort episodic memories chronologically, falling back to identifiers.
pub(crate) fn sort_memories(memories: &mut [EpisodicMemory]) {
    memories.sort_by(
        |left, right| match (&left.parsed_timestamp, &right.parsed_timestamp) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.memory_id.cmp(&right.memory_id),
        },
    );
}

/// Compute a deterministic hash used as the summary idempotency key.
pub(crate) fn compute_summary_key(
    project_id: &str,
    time_range: &SearchTimeRange,
    source_memory_ids: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project_id.as_bytes());
    let start = time_range.start.as_deref().unwrap_or("");
    let end = time_range.end.as_deref().unwrap_or("");
    hasher.update(start.as_bytes());
    hasher.update(end.as_bytes());
    for id in source_memory_ids {
        hasher.update(id.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Build the abstractive summarization prompt.
pub(crate) fn build_abstractive_prompt(
    project_id: &str,
    time_range: &SearchTimeRange,
    max_words: usize,
    memories: &[EpisodicMemory],
) -> String {
    let start = time_range.start.as_deref().unwrap_or("(unspecified)");
    let end = time_range.end.as_deref().unwrap_or("(unspecified)");
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "System: You summarize developer activity into concise, factual bullet points. Prefer neutral tone. Avoid speculation. Include dates if present. Return at most {max_words} words. Output a single paragraph.\n\n"
    ));
    prompt.push_str(&format!(
        "Summarize the following episodic notes for project '{project_id}' between {start} and {end}.\n"
    ));

    for memory in memories {
        let text = memory.text.trim();
        if text.is_empty() {
            continue;
        }
        let snippet = truncate_sentence(text, 180);
        if let Some(timestamp) = memory.timestamp.as_deref() {
            prompt.push_str(&format!("- {timestamp}: {snippet}\n"));
        } else {
            prompt.push_str(&format!("- {snippet}\n"));
        }
    }

    prompt
}

/// Build a deterministic extractive summary bounded by a word budget.
pub(crate) fn build_extractive_summary(memories: &[EpisodicMemory], max_words: usize) -> String {
    let mut bullets = Vec::new();
    let mut used_words = 0usize;

    for memory in memories {
        let text = memory.text.trim();
        if text.is_empty() {
            continue;
        }

        let sentence = truncate_sentence(first_sentence(text), 180);
        if sentence.is_empty() {
            continue;
        }

        let bullet = if let Some(timestamp) = memory.timestamp.as_deref() {
            format!("- {}: {}", timestamp, sentence)
        } else {
            format!("- {}", sentence)
        };

        let bullet_words = count_words(&bullet);
        if bullet_words == 0 {
            continue;
        }
        if !bullets.is_empty() && used_words + bullet_words > max_words {
            break;
        }
        used_words += bullet_words;
        bullets.push(bullet);
        if used_words >= max_words {
            break;
        }
    }

    if bullets.is_empty() {
        return memories
            .iter()
            .find_map(|memory| {
                let trimmed = memory.text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(truncate_sentence(trimmed, 200))
                }
            })
            .unwrap_or_else(|| "No episodic memories available.".into());
    }

    bullets.join("\n")
}

fn first_sentence(text: &str) -> &str {
    text.split(|c| matches!(c, '.' | '!' | '?'))
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .unwrap_or(text)
}

fn truncate_sentence(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.chars().take(max_chars - 1).collect::<String>();
    truncated.push('…');
    truncated
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_key_is_deterministic() {
        let range = SearchTimeRange {
            start: Some("2025-01-01T00:00:00Z".into()),
            end: Some("2025-01-07T00:00:00Z".into()),
        };
        let ids = vec!["a".into(), "b".into()];
        let key1 = compute_summary_key("default", &range, &ids);
        let key2 = compute_summary_key("default", &range, &ids);
        assert_eq!(key1, key2);
        assert!(!key1.is_empty());
    }

    #[test]
    fn sort_memories_orders_by_timestamp() {
        let mut memories = vec![
            EpisodicMemory::new(
                "2".into(),
                "Later".into(),
                Some("2025-01-02T00:00:00Z".into()),
            ),
            EpisodicMemory::new(
                "1".into(),
                "Earlier".into(),
                Some("2025-01-01T00:00:00Z".into()),
            ),
        ];
        sort_memories(&mut memories);
        assert_eq!(memories[0].memory_id, "1");
    }

    #[test]
    fn extractive_summary_respects_word_budget() {
        let memories = vec![
            EpisodicMemory::new(
                "1".into(),
                "Implemented login flow. Fixed bugs.".into(),
                Some("2025-01-01".into()),
            ),
            EpisodicMemory::new(
                "2".into(),
                "Added search endpoint.".into(),
                Some("2025-01-02".into()),
            ),
        ];
        let summary = build_extractive_summary(&memories, 6);
        let word_count = count_words(&summary);
        assert!(word_count <= 6);
        assert!(summary.contains("2025-01-01"));
    }
}
