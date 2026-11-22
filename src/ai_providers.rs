use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint, ProviderConfig};
use genai::Client;
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

/// Discovers model IDs through a configured provider connection. Discovery is
/// advisory: errors are returned for the UI to display and never replace the
/// existing manual/shipped suggestions.
pub async fn discover_models(
    adapter_kind: AdapterKind,
    endpoint: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let endpoint = Endpoint::from_owned(endpoint);
    let auth = api_key.map_or(AuthData::None, AuthData::from_single);
    let client = Client::builder()
        .with_adapter_kind(adapter_kind)
        .append_provider_config(adapter_kind, ProviderConfig::from((endpoint, auth)))
        .build()
        .map_err(|error| format!("Could not initialize model discovery: {error}"))?;
    let models = client
        .all_model_names(adapter_kind, None)
        .await
        .map_err(|error| format!("Could not discover models: {error}"))?;
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for model in models {
        let model = model.trim();
        if !model.is_empty()
            && model.len() <= crate::ai_config::MAX_MODEL_ID_BYTES
            && seen.insert(model.to_lowercase())
        {
            names.push(model.to_owned());
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

pub fn merge_model_suggestions(
    adapter_kind: AdapterKind,
    discovered: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut models = discovered;
    for provider in built_in_providers()?
        .iter()
        .filter(|provider| provider.adapter_kind == adapter_kind)
    {
        models.extend(provider.models.iter().cloned());
    }
    let mut seen = std::collections::HashSet::new();
    models.retain(|model| seen.insert(model.to_lowercase()));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
    use tokio::net::TcpListener;

    async fn models_response(State(auth): State<String>) -> impl IntoResponse {
        (
            StatusCode::OK,
            [("x-test-auth", auth)],
            axum::Json(serde_json::json!({
                "data": [
                    {"id": "zeta-model"},
                    {"id": "alpha-model"},
                    {"id": "alpha-model"}
                ]
            })),
        )
            .into_response()
    }

    async fn start_models_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/models", get(models_response))
            .with_state("test-key".to_owned());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/v1/"), task)
    }

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

    #[tokio::test]
    async fn discovery_should_return_sorted_unique_model_ids() {
        let (url, server) = start_models_server().await;

        let models = discover_models(AdapterKind::OpenAI, url, Some("test-key".to_owned()))
            .await
            .unwrap();
        server.abort();

        assert_eq!(models, ["alpha-model", "zeta-model"]);
    }

    #[test]
    fn merging_should_preserve_discovered_models_and_add_provider_suggestions() {
        let models =
            merge_model_suggestions(AdapterKind::OpenAI, vec!["custom-model".to_owned()]).unwrap();

        assert_eq!(models.first().map(String::as_str), Some("custom-model"));
        assert!(models.len() > 1);
    }
}
