use genai::adapter::AdapterKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub name: String,
    pub api_url: String,
    pub adapter_kind: AdapterKind,
    pub models: Vec<String>,
}

pub fn built_in_providers() -> Result<Vec<Provider>, String> {
    let providers: Vec<Provider> = serde_json::from_str(include_str!("../data/ai-providers.json"))
        .map_err(|error| format!("Could not load built-in AI providers: {error}"))?;
    let mut names = std::collections::HashSet::new();
    for provider in &providers {
        if provider.name.trim().is_empty()
            || !crate::ai_config::is_supported_interaction_method(provider.adapter_kind)
            || !names.insert(provider.name.to_lowercase())
        {
            return Err("Built-in AI provider data is invalid.".to_owned());
        }
        crate::ai_config::normalize_url(&provider.api_url).map_err(|error| {
            format!(
                "Invalid URL for built-in provider {}: {error}",
                provider.name
            )
        })?;
        if provider.models.iter().any(|model| {
            model.trim().is_empty() || model.len() > crate::ai_config::MAX_MODEL_ID_BYTES
        }) {
            return Err(format!(
                "Invalid model suggestions for built-in provider {}.",
                provider.name
            ));
        }
    }
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_catalog_has_unique_named_providers_and_known_methods() {
        let providers = built_in_providers().unwrap();
        assert!(providers.iter().any(|provider| {
            provider.name == "OpenAI: Chat Completions API"
                && provider.adapter_kind == AdapterKind::OpenAI
        }));
        assert!(providers.iter().any(|provider| {
            provider.name == "OpenAI: Responses API"
                && provider.adapter_kind == AdapterKind::OpenAIResp
        }));
    }
}
