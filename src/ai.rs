use std::{fs, path::PathBuf, sync::RwLock};

use directories::ProjectDirs;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

pub const MAX_INPUT_TOKENS: usize = 16_000;
pub const MAX_OUTPUT_TOKENS: u32 = 512;
pub const MAX_HISTORY_MESSAGES: usize = 20;

const SYSTEM_PROMPT: &str = "You are a concise Dwarf Fortress companion. Answer the player's question using the supplied fortress reports as evidence. Distinguish observed facts from uncertainty, and do not give unsolicited advice.";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenAi,
    Groq,
    OpenAiCompatible,
}

impl Provider {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Groq => "Groq",
            Self::OpenAiCompatible => "OpenAI-compatible provider",
        }
    }

    pub fn privacy_policy_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("https://www.anthropic.com/legal/privacy"),
            Self::OpenAi => Some("https://openai.com/policies/privacy-policy/"),
            Self::Groq => Some("https://groq.com/privacy-policy/"),
            Self::OpenAiCompatible => None,
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::OpenAiCompatible => "",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }

    fn credential_account(self, base_url: &str) -> String {
        match self {
            Self::Anthropic => "anthropic".to_owned(),
            Self::OpenAi => "openai".to_owned(),
            Self::Groq => "groq".to_owned(),
            Self::OpenAiCompatible => {
                let digest = Sha256::digest(base_url.as_bytes());
                format!("openai-compatible-{}", hex_string(&digest))
            }
        }
    }

    fn uses_openai_compatible_api(self) -> bool {
        matches!(self, Self::OpenAi | Self::Groq | Self::OpenAiCompatible)
    }
}

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserSettings {
    pub provider: Provider,
    pub model_id: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
}

fn default_openai_base_url() -> String {
    Provider::OpenAi.default_base_url().to_owned()
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Anthropic,
            model_id: "claude-sonnet-5-5".to_owned(),
            base_url: Provider::OpenAi.default_base_url().to_owned(),
        }
    }
}

pub struct SettingsRepository {
    path: Option<PathBuf>,
    settings: RwLock<UserSettings>,
}

impl SettingsRepository {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            settings: RwLock::new(UserSettings::default()),
        }
    }

    pub fn load_default() -> Result<Self, String> {
        let dirs = ProjectDirs::from("org", "DFCompanion", "DFCompanion")
            .ok_or_else(|| "Could not locate the per-user settings directory.".to_owned())?;
        let path = dirs.data_local_dir().join("settings.json");
        Self::load(path)
    }

    fn load(path: PathBuf) -> Result<Self, String> {
        let settings = match fs::read(&path) {
            Ok(contents) => serde_json::from_slice(&contents)
                .map_err(|error| format!("Could not read AI settings: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => UserSettings::default(),
            Err(error) => return Err(format!("Could not read AI settings: {error}")),
        };
        Ok(Self {
            path: Some(path),
            settings: RwLock::new(settings),
        })
    }

    pub fn get(&self) -> Result<UserSettings, String> {
        self.settings
            .read()
            .map(|settings| settings.clone())
            .map_err(|_| "AI settings are unavailable.".to_owned())
    }

    pub fn update(
        &self,
        provider: Provider,
        model_id: String,
        base_url: String,
    ) -> Result<UserSettings, String> {
        let mut current = self
            .settings
            .write()
            .map_err(|_| "AI settings are unavailable.".to_owned())?;
        let base_url = if provider.uses_openai_compatible_api() {
            base_url
        } else {
            provider.default_base_url().to_owned()
        };
        let next = UserSettings {
            provider,
            model_id,
            base_url,
        };
        self.persist(&next)?;
        *current = next.clone();
        Ok(next)
    }

    fn persist(&self, settings: &UserSettings) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .ok_or_else(|| "Could not locate the AI settings directory.".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the AI settings directory: {error}"))?;
        let serialized = serde_json::to_vec_pretty(settings)
            .map_err(|error| format!("Could not serialize AI settings: {error}"))?;
        let temporary_path = path.with_extension("json.tmp");
        fs::write(&temporary_path, serialized)
            .map_err(|error| format!("Could not save AI settings: {error}"))?;
        fs::rename(&temporary_path, path)
            .map_err(|error| format!("Could not save AI settings: {error}"))
    }
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
}

impl ChatRole {
    fn anthropic_name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn openai_name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Default)]
pub struct Conversation {
    pub history: Vec<ChatMessage>,
    pub pending_reports: Vec<String>,
    pub pending_user_messages: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub history: Vec<ChatMessage>,
    pub current_user_content: String,
    pub omitted_report_count: usize,
    pub included_user_message_count: usize,
}

impl Conversation {
    pub fn push_report(&mut self, report: String) -> usize {
        self.pending_reports.push(report);
        let mut dropped = 0;
        while estimate_prompt_tokens(&[], &format_prompt_content(&self.pending_reports, ""))
            > MAX_INPUT_TOKENS
        {
            self.pending_reports.remove(0);
            dropped += 1;
        }
        dropped
    }

    pub fn build_prompt(&self) -> Prompt {
        let mut history = self.history.clone();
        let mut reports = self.pending_reports.clone();
        let mut omitted_report_count = 0;
        let mut included_user_message_count = self.pending_user_messages.len();

        loop {
            let player_messages =
                self.pending_user_messages[..included_user_message_count].join("\n");
            let current_user_content = format_prompt_content(&reports, &player_messages);
            if estimate_prompt_tokens(&history, &current_user_content) <= MAX_INPUT_TOKENS {
                return Prompt {
                    history,
                    current_user_content,
                    omitted_report_count,
                    included_user_message_count,
                };
            }

            if !reports.is_empty() {
                reports.remove(0);
                omitted_report_count += 1;
            } else if history.len() >= 2 {
                history.drain(0..2);
            } else if included_user_message_count > 1 {
                included_user_message_count -= 1;
            } else {
                return Prompt {
                    history,
                    current_user_content,
                    omitted_report_count,
                    included_user_message_count,
                };
            }
        }
    }

    pub fn complete(&mut self, prompt: Prompt, answer: String) {
        let Prompt {
            history,
            current_user_content,
            included_user_message_count,
            ..
        } = prompt;
        self.history = history;
        self.history.push(ChatMessage {
            role: ChatRole::User,
            content: current_user_content,
        });
        self.history.push(ChatMessage {
            role: ChatRole::Assistant,
            content: answer,
        });
        if self.history.len() > MAX_HISTORY_MESSAGES {
            let excess = self.history.len() - MAX_HISTORY_MESSAGES;
            self.history.drain(0..excess);
        }
        self.pending_reports.clear();
        self.pending_user_messages
            .drain(0..included_user_message_count);
    }
}

fn format_prompt_content(reports: &[String], player_messages: &str) -> String {
    let mut content = String::new();
    if !reports.is_empty() {
        content.push_str("Fortress reports received since the previous answer:\n");
        for report in reports {
            content.push_str("- ");
            content.push_str(report);
            content.push('\n');
        }
    }
    content.push_str("\nPlayer message(s):\n");
    content.push_str(player_messages);
    content
}

fn estimate_prompt_tokens(history: &[ChatMessage], current_user_content: &str) -> usize {
    let text_size = SYSTEM_PROMPT.len()
        + history
            .iter()
            .map(|message| message.content.len())
            .sum::<usize>()
        + current_user_content.len();
    let message_overhead = history.len().saturating_add(1).saturating_mul(8);
    text_size.saturating_add(2) / 3 + message_overhead
}

pub struct AiRuntime {
    pub settings: SettingsRepository,
    pub conversation: Mutex<Conversation>,
    client: Client,
}

impl AiRuntime {
    pub fn new(settings: SettingsRepository) -> Self {
        Self {
            settings,
            conversation: Mutex::new(Conversation::default()),
            client: Client::new(),
        }
    }

    pub fn has_api_key(&self, provider: Provider, base_url: &str) -> Result<bool, String> {
        match get_api_key(provider, base_url) {
            Ok(_) => Ok(true),
            Err(CredentialError::Missing) => Ok(false),
            Err(CredentialError::Unavailable) => {
                Err("The operating system credential vault is unavailable.".to_owned())
            }
        }
    }

    pub fn save_api_key(
        &self,
        provider: Provider,
        base_url: &str,
        key: &str,
    ) -> Result<(), String> {
        let entry = credential_entry(provider, base_url)?;
        entry.set_password(key).map_err(|_| {
            "Could not save the API key to the operating system credential vault.".to_owned()
        })
    }

    pub fn forget_api_key(&self, provider: Provider, base_url: &str) -> Result<(), String> {
        let entry = credential_entry(provider, base_url)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(
                "Could not remove the API key from the operating system credential vault."
                    .to_owned(),
            ),
        }
    }

    pub async fn complete(
        &self,
        settings: &UserSettings,
        prompt: &Prompt,
    ) -> Result<String, String> {
        if settings.provider == Provider::OpenAiCompatible {
            validate_compatible_base_url(&settings.base_url)?;
        }
        let key = match get_api_key(settings.provider, &settings.base_url) {
            Ok(key) => key,
            Err(CredentialError::Missing) => {
                return Err(format!(
                    "No {} API key is saved. Add one in AI Provider settings.",
                    settings.provider.display_name()
                ));
            }
            Err(CredentialError::Unavailable) => {
                return Err("The operating system credential vault is unavailable.".to_owned());
            }
        };

        if settings.model_id.trim().is_empty() {
            return Err("Enter a model ID in AI Provider settings.".to_owned());
        }
        let response = match settings.provider {
            Provider::Anthropic => self.anthropic(settings, prompt, &key).await?,
            Provider::OpenAi | Provider::Groq | Provider::OpenAiCompatible => {
                self.openai_compatible(settings, prompt, &key).await?
            }
        };
        Ok(response)
    }

    async fn anthropic(
        &self,
        settings: &UserSettings,
        prompt: &Prompt,
        key: &str,
    ) -> Result<String, String> {
        let messages = prompt_messages(prompt)
            .into_iter()
            .map(|(role, content)| json!({"role": role.anthropic_name(), "content": content}))
            .collect::<Vec<_>>();
        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .timeout(std::time::Duration::from_secs(60))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": settings.model_id,
                "max_tokens": MAX_OUTPUT_TOKENS,
                "system": SYSTEM_PROMPT,
                "messages": messages,
            }))
            .send()
            .await
            .map_err(|error| format!("Anthropic request failed: {error}"))?;
        let body = checked_response(response, "Anthropic").await?;
        let text = body["content"]
            .as_array()
            .and_then(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .next()
            })
            .ok_or_else(|| "Anthropic returned no text response.".to_owned())?;
        Ok(text.to_owned())
    }

    async fn openai_compatible(
        &self,
        settings: &UserSettings,
        prompt: &Prompt,
        key: &str,
    ) -> Result<String, String> {
        let mut messages = vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
        messages.extend(
            prompt_messages(prompt)
                .into_iter()
                .map(|(role, content)| json!({"role": role.openai_name(), "content": content})),
        );
        let url = openai_compatible_chat_url(&settings.base_url)?;
        let response = self
            .client
            .post(url)
            .timeout(std::time::Duration::from_secs(60))
            .bearer_auth(key)
            .json(&json!({
                "model": settings.model_id,
                "max_completion_tokens": MAX_OUTPUT_TOKENS,
                "messages": messages,
            }))
            .send()
            .await
            .map_err(|error| {
                format!(
                    "{} request failed: {error}",
                    settings.provider.display_name()
                )
            })?;
        let body = checked_response(response, settings.provider.display_name()).await?;
        let text = body["choices"]
            .as_array()
            .and_then(|choices| choices.first())
            .and_then(|choice| choice["message"]["content"].as_str())
            .ok_or_else(|| {
                format!(
                    "{} returned no text response.",
                    settings.provider.display_name()
                )
            })?;
        Ok(text.to_owned())
    }
}

fn prompt_messages(prompt: &Prompt) -> Vec<(ChatRole, &str)> {
    let mut messages = prompt
        .history
        .iter()
        .map(|message| (message.role, message.content.as_str()))
        .collect::<Vec<_>>();
    messages.push((ChatRole::User, prompt.current_user_content.as_str()));
    messages
}

async fn checked_response(response: reqwest::Response, provider: &str) -> Result<Value, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{provider} response could not be read: {error}"))?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value["error"]["message"]
                    .as_str()
                    .or_else(|| value["error"]["type"].as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| body.chars().take(500).collect());
        return Err(format!(
            "{provider} returned HTTP {}: {detail}",
            status.as_u16()
        ));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("{provider} returned invalid JSON: {error}"))
}

fn get_api_key(provider: Provider, base_url: &str) -> Result<String, CredentialError> {
    let entry = keyring::Entry::new("DFCompanion", &provider.credential_account(base_url))
        .map_err(|_| CredentialError::Unavailable)?;
    match entry.get_password() {
        Ok(key) => Ok(key),
        Err(keyring::Error::NoEntry) => Err(CredentialError::Missing),
        Err(_) => Err(CredentialError::Unavailable),
    }
}

enum CredentialError {
    Missing,
    Unavailable,
}

fn credential_entry(provider: Provider, base_url: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("DFCompanion", &provider.credential_account(base_url))
        .map_err(|_| "Could not access the operating system credential vault.".to_owned())
}

fn openai_compatible_chat_url(base_url: &str) -> Result<Url, String> {
    validate_compatible_base_url(base_url)?;
    let mut url = Url::parse(base_url)
        .map_err(|error| format!("Invalid OpenAI-compatible base URL: {error}"))?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| "Invalid OpenAI-compatible base URL.".to_owned())?;
    segments.pop_if_empty();
    segments.push("chat");
    segments.push("completions");
    drop(segments);
    Ok(url)
}

pub fn validate_compatible_base_url(base_url: &str) -> Result<(), String> {
    let url = Url::parse(base_url.trim())
        .map_err(|_| "Enter a valid HTTPS base URL for the OpenAI-compatible API.".to_owned())?;
    let local_http = url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !local_http {
        return Err(
            "The OpenAI-compatible base URL must use HTTPS (HTTP is allowed only for localhost)."
                .to_owned(),
        );
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "The OpenAI-compatible base URL must not include credentials, a query, or a fragment."
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_endpoint_builds_chat_completion_url_and_rejects_insecure_urls() {
        assert_eq!(
            openai_compatible_chat_url("https://api.groq.com/openai/v1")
                .unwrap()
                .as_str(),
            "https://api.groq.com/openai/v1/chat/completions"
        );
        assert_eq!(
            openai_compatible_chat_url("http://localhost:1234/v1/")
                .unwrap()
                .as_str(),
            "http://localhost:1234/v1/chat/completions"
        );
        assert!(validate_compatible_base_url("http://example.com/v1").is_err());
        assert!(validate_compatible_base_url("https://user@example.com/v1").is_err());
        assert!(validate_compatible_base_url("https://example.com/v1?key=x").is_err());
    }

    #[test]
    fn compatible_endpoints_use_distinct_credential_accounts() {
        assert_ne!(
            Provider::OpenAiCompatible.credential_account("https://one.example/v1"),
            Provider::OpenAiCompatible.credential_account("https://two.example/v1")
        );
        assert_eq!(
            Provider::OpenAi.credential_account(Provider::OpenAi.default_base_url()),
            "openai"
        );
    }

    #[test]
    fn conversation_drops_old_reports_before_older_chat_history() {
        let mut conversation = Conversation::default();
        conversation.history = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "old conversation ".repeat(50_000),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "old answer ".repeat(50_000),
            },
        ];
        conversation.pending_reports = vec!["recent report".to_owned()];
        conversation.pending_user_messages = vec!["What happened?".to_owned()];

        let prompt = conversation.build_prompt();

        assert_eq!(prompt.omitted_report_count, 1);
        assert!(prompt.history.is_empty());
        assert!(prompt.current_user_content.contains("What happened?"));
        assert!(
            estimate_prompt_tokens(&prompt.history, &prompt.current_user_content)
                <= MAX_INPUT_TOKENS
        );
    }

    #[test]
    fn successful_turn_is_bounded_and_clears_pending_context() {
        let mut conversation = Conversation::default();
        conversation.pending_reports.push("report".to_owned());
        conversation
            .pending_user_messages
            .push("question".to_owned());
        let prompt = conversation.build_prompt();

        conversation.complete(prompt, "answer".to_owned());

        assert!(conversation.pending_reports.is_empty());
        assert!(conversation.pending_user_messages.is_empty());
        assert_eq!(conversation.history.len(), 2);
    }

    #[test]
    fn pending_reports_are_bounded_before_a_player_turn() {
        let mut conversation = Conversation::default();
        let dropped = conversation.push_report("old report ".repeat(15_000));
        let following_dropped = conversation.push_report("latest report".to_owned());

        assert_eq!(dropped, 1);
        assert_eq!(following_dropped, 0);
        assert_eq!(conversation.pending_reports, vec!["latest report"]);
    }

    #[test]
    fn settings_file_contains_no_credential_data() {
        let path = std::env::temp_dir().join(format!(
            "dfcompanion-settings-test-{}.json",
            std::process::id()
        ));
        let repository = SettingsRepository::load(path.clone()).unwrap();
        repository
            .update(
                Provider::OpenAiCompatible,
                "custom-model".to_owned(),
                "https://example.com/v1".to_owned(),
            )
            .unwrap();
        repository
            .update(
                Provider::OpenAi,
                "openai-example".to_owned(),
                Provider::OpenAi.default_base_url().to_owned(),
            )
            .unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("openai-example"));
        assert!(!contents.contains("gemini-example"));
        assert!(!contents.contains("api_key"));
        assert!(!contents.contains("acknowledged"));
        fs::remove_file(path).unwrap();
    }
}
