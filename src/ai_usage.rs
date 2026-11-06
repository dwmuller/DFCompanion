use crate::ai_tools::definitions;
use genai::chat::Usage;
use tokenx_rs::estimate_token_count;

#[derive(Clone, Copy, Debug, Default)]
pub struct AiPromptTokenEstimates {
    pub pre_trim: bool,
    pub system_text_tokens: usize,
    pub reports_tokens: usize,
    pub user_messages_tokens: usize,
    pub history_tokens: usize,
    pub tools_tokens: usize,
    pub total_tokens: usize,
}

impl AiPromptTokenEstimates {
    pub fn new(
        pre_trim: bool,
        system_text: &str,
        reports: &[String],
        user_messages: &[String],
        history: &[String],
    ) -> Self {
        let system_text_tokens = estimate_token_count(system_text);
        let reports_tokens = reports
            .iter()
            .map(|r| estimate_token_count(&r))
            .sum::<usize>();
        let user_messages_tokens = user_messages
            .iter()
            .map(|m| estimate_token_count(&m))
            .sum::<usize>();
        let history_tokens = history
            .iter()
            .map(|hist| estimate_token_count(&hist))
            .sum::<usize>();
        let tools_tokens = definitions()
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
            .sum();
        let total_tokens = system_text_tokens
            + reports_tokens
            + user_messages_tokens
            + history_tokens
            + tools_tokens;
        AiPromptTokenEstimates {
            pre_trim,
            system_text_tokens,
            reports_tokens,
            user_messages_tokens,
            history_tokens,
            tools_tokens,
            total_tokens,
        }
    }
    pub fn summary(&self) -> String {
        format!(
            "Prompt {} token estimates: system {}, reports {}, user {}, history {}, tools {}, total {}",
            if self.pre_trim { "pre-trim" } else { "post-trim" },
            self.system_text_tokens,
            self.reports_tokens,
            self.user_messages_tokens,
            self.history_tokens,
            self.tools_tokens,
            self.total_tokens
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct AiTurnUsage {
    pub purpose: String,
    pub prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_reported: Option<u64>,
}

impl AiTurnUsage {
    pub fn from_genai_usage(usage: &Usage, purpose: &str) -> Self {
        AiTurnUsage {
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

    pub(crate) fn total_tokens(&self) -> Option<u64> {
        if self.prompt_tokens.is_none() && self.completion_tokens.is_none() {
            return None;
        }
        Some(self.prompt_tokens.unwrap_or(0) + self.completion_tokens.unwrap_or(0))
    }
    pub(crate) fn summary(&self) -> String {
        format!(
            "Tokens for {}:\n  {:>6} input ({:>6} [{:>2}%] cached, {:>6} cache creation), {:>6} output, {:>7} total",
            self.purpose,
            self.prompt_tokens.map(|t| t.to_string()).unwrap_or("unreported".to_string()),
            self.cached_prompt_tokens.map(|t| t.to_string()).unwrap_or("unreported".to_string()),
            if let (Some(cached), Some(total)) = (self.cached_prompt_tokens, self.prompt_tokens) {
                (cached * 100 / total).to_string()
            } else {
                "unreported".to_string()
            },
            self.cache_creation_tokens.map(|t| t.to_string()).unwrap_or("unreported".to_string()),
            self.completion_tokens.map(|t| t.to_string()).unwrap_or("unreported".to_string()),
            self.total_tokens().map(|t| t.to_string()).unwrap_or("unreported".to_string()),
        )
    }
}
