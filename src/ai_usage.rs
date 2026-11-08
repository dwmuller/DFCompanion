use crate::ai_tools::definitions;
use genai::chat::Usage;
use tokenx_rs::estimate_token_count;

#[derive(Clone, Copy, Debug, Default)]
pub struct AiPromptTokenEstimates {
    pub system_text_tokens: usize,
    pub content_tokens: usize,
    pub history_tokens: usize,
    pub tools_tokens: usize,
    pub total_tokens: usize,
}

pub fn tools_tokens_estimate() -> usize {
    definitions()
        .iter()
        .map(|def| {
            def.schema
                .as_ref()
                .map_or(0, |s| s.as_str().map_or(0, |s| estimate_token_count(s)))
                + def
                    .description
                    .as_ref()
                    .map_or(0, |d| estimate_token_count(&d))
        })
        .sum::<usize>()
}

#[derive(Clone, Debug, Default)]
pub struct AiTurnUsage {
    pub purpose: String,
    pub estimated_prompt_tokens: usize,
    pub prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_reported: Option<u64>,
}

impl AiTurnUsage {
    pub fn from_genai_usage(estimated_prompt_tokens: usize, usage: &Usage, purpose: &str) -> Self {
        AiTurnUsage {
            estimated_prompt_tokens: estimated_prompt_tokens,
            prompt_tokens: usage.prompt_tokens.map(|t| t as u64),
            cached_prompt_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens.map(|t| t as u64)),
            cache_creation_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cache_creation_tokens.map(|t| t as u64)),
            completion_tokens: usage.completion_tokens.map(|t| t as u64),
            total_reported: usage.total_tokens.map(|t| t as u64),
            purpose: purpose.to_string(),
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "Tokens for {}:\n  {:>6} est, {:>6} input ({:>6} [{:>2}%] cached, {:>6} cache creation), {:>6} output, {:>7} total [{:>3}%]",
            self.purpose,
            self.estimated_prompt_tokens,
            self.prompt_tokens.map(|t| t.to_string()).unwrap_or("n/a".to_string()),
            self.cached_prompt_tokens.map(|t| t.to_string()).unwrap_or("n/a".to_string()),
            if let (Some(cached), Some(total)) = (self.cached_prompt_tokens, self.prompt_tokens) {
                (cached * 100 / total).to_string()
            } else {
                "unreported".to_string()
            },
            self.cache_creation_tokens.map(|t| t.to_string()).unwrap_or("n/a".to_string()),
            self.completion_tokens.map(|t| t.to_string()).unwrap_or("n/a".to_string()),
            self.total_reported.map(|t| t.to_string()).unwrap_or("n/a".to_string()),
            self.total_reported.map_or("n/a".to_string(), |t| (t * 100 / self.estimated_prompt_tokens as u64).to_string()),
        )
    }
}
