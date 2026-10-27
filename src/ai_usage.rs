use genai::chat::Usage;

#[derive(Clone, Copy, Debug, Default)]
pub struct AiTurnUsage {
    pub request_count: u32,
    pub prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_reported: Option<u64>,
}

impl AiTurnUsage {

    pub(crate) fn add(&mut self, other: &Usage) {
        self.request_count += 1;
        if let Some(prompt_tokens) = other.prompt_tokens {
            self.prompt_tokens = Some(self.prompt_tokens.unwrap_or(0) + prompt_tokens as u64);
        }
        if let Some(details) = &other.prompt_tokens_details {
            if let Some(cached) = details.cached_tokens {
                self.cached_prompt_tokens = Some(self.cached_prompt_tokens.unwrap_or(0) + cached as u64);
            }
            if let Some(cache_creation) = details.cache_creation_tokens {
                self.cache_creation_tokens = Some(self.cache_creation_tokens.unwrap_or(0) + cache_creation as u64);
            }
        }
        if let Some(completion_tokens) = other.completion_tokens {
            self.completion_tokens = Some(self.completion_tokens.unwrap_or(0) + completion_tokens as u64);
        }
        if let Some(total_reported) = other.total_tokens {
            self.total_reported = Some(self.total_reported.unwrap_or(0) + total_reported as u64);
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
            "This turn: {} API request{}, {} input tokens ({} ({}%) cached, {} cache creation), {} output tokens, {} total tokens",
            self.request_count,
            if self.request_count == 1 { "" } else { "s" },
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

