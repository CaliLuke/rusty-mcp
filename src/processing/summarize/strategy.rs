use crate::config::{Config, SummarizationProvider};
use crate::processing::{SummarizeRequest, SummarizeStrategy};
use crate::summarization::{
    SummarizationRequest as LlmSummarizationRequest, get_summarization_client,
};

use super::{EpisodicMemory, build_abstractive_prompt, build_extractive_summary};

/// Result of applying a summarization strategy selection.
pub(crate) struct StrategyResult {
    pub summary_text: String,
    pub strategy: SummarizeStrategy,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Choose between abstractive and extractive summarization.
pub(crate) async fn select_summary_strategy(
    request: &SummarizeRequest,
    items: &[EpisodicMemory],
    config: &Config,
) -> StrategyResult {
    let mut chosen_strategy = request.strategy.clone().unwrap_or(SummarizeStrategy::Auto);
    let mut provider = request.provider.clone();
    let mut model = request.model.clone();
    let mut summary_text = String::new();

    if matches!(
        chosen_strategy,
        SummarizeStrategy::Auto | SummarizeStrategy::Abstractive
    ) && matches!(config.summarization_provider, SummarizationProvider::Ollama)
    {
        if model.is_none() {
            model = config.summarization_model.clone();
        }
        if provider.is_none() {
            provider = Some("ollama".into());
        }
        if let (Some(model_name), Some(client)) = (model.clone(), get_summarization_client()) {
            let max_words = request.max_words.unwrap_or(config.summarization_max_words);
            let prompt = build_abstractive_prompt(
                request.project_id.as_deref().unwrap_or("default"),
                &request.time_range,
                max_words,
                items,
            );
            match client
                .generate_summary(LlmSummarizationRequest {
                    model: model_name.clone(),
                    prompt,
                    max_words,
                })
                .await
            {
                Ok(text) => {
                    summary_text = text;
                    chosen_strategy = SummarizeStrategy::Abstractive;
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "Abstractive summarization failed; falling back to extractive"
                    );
                }
            }
        }
    }

    if summary_text.is_empty() {
        let max_words = request.max_words.unwrap_or(config.summarization_max_words);
        summary_text = build_extractive_summary(items, max_words);
        if matches!(chosen_strategy, SummarizeStrategy::Auto) {
            chosen_strategy = SummarizeStrategy::Extractive;
        }
    }

    StrategyResult {
        summary_text,
        strategy: chosen_strategy,
        provider,
        model,
    }
}
