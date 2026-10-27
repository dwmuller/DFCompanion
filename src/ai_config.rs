use std::{
    fs,
    path::PathBuf,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use directories::ProjectDirs;
use genai::adapter::AdapterKind;
use keyring::Entry;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use url::Url;

const CONFIG_FILE_NAME: &str = "ai-configurations.json";
const MAX_NAME_BYTES: usize = 128;
pub(crate) const MAX_MODEL_ID_BYTES: usize = 256;
const MAX_URL_BYTES: usize = 512;
pub const MAX_SYSTEM_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_API_KEY_BYTES: usize = 4_096;

pub fn is_supported_interaction_method(method: AdapterKind) -> bool {
    interaction_method_label(method).is_some()
}

pub fn interaction_method_label(method: AdapterKind) -> Option<&'static str> {
    // This is the definitive list of AdapterKind values that we support, as well
    // as providing human-readable labels for them.
    match method {
        AdapterKind::OpenAI => Some("OpenAI Chat Completions"),
        AdapterKind::OpenAIResp => Some("OpenAI Responses"),
        AdapterKind::Anthropic => Some("Anthropic"),
        _ => None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiConfiguration {
    pub id: String,
    pub name: String,
    #[serde(alias = "method", deserialize_with = "deserialize_interaction_method")]
    pub interaction_method: AdapterKind,
    pub model_id: String,
    pub url: String,
    pub system_text: String,
}

pub(crate) fn deserialize_interaction_method<'de, D>(
    deserializer: D,
) -> Result<AdapterKind, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "OpenAI" | "open_ai_chat_completions" => Ok(AdapterKind::OpenAI),
        "OpenAIResp" | "open_ai_responses" => Ok(AdapterKind::OpenAIResp),
        "Anthropic" | "anthropic" => Ok(AdapterKind::Anthropic),
        _ => Err(serde::de::Error::custom(
            "unsupported AI interaction method",
        )),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationFile {
    configurations: Vec<AiConfiguration>,
    active_id: Option<String>,
}

impl Default for ConfigurationFile {
    fn default() -> Self {
        Self {
            configurations: Vec::new(),
            active_id: None,
        }
    }
}

pub struct ConfigurationRepository {
    path: Option<PathBuf>,
    configurations: RwLock<ConfigurationFile>,
}

impl ConfigurationRepository {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            configurations: RwLock::new(ConfigurationFile::default()),
        }
    }

    pub fn load_default() -> Result<Self, String> {
        let dirs = ProjectDirs::from("org", "DFCompanion", "DFCompanion")
            .ok_or_else(|| "Could not locate the per-user settings directory.".to_owned())?;
        Self::load(dirs.data_local_dir().join(CONFIG_FILE_NAME))
    }

    fn load(path: PathBuf) -> Result<Self, String> {
        let configurations = match fs::read(&path) {
            Ok(contents) => serde_json::from_slice::<ConfigurationFile>(&contents)
                .map_err(|error| format!("Could not read AI configurations: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ConfigurationFile::default()
            }
            Err(error) => return Err(format!("Could not read AI configurations: {error}")),
        };
        validate_file(&configurations)?;
        Ok(Self {
            path: Some(path),
            configurations: RwLock::new(configurations),
        })
    }

    pub fn list(&self) -> Result<(Vec<AiConfiguration>, Option<String>), String> {
        let configurations = self.read()?;
        Ok((
            configurations.configurations.clone(),
            configurations.active_id.clone(),
        ))
    }

    pub fn get(&self, id: &str) -> Result<AiConfiguration, String> {
        self.read()?
            .configurations
            .iter()
            .find(|configuration| configuration.id == id)
            .cloned()
            .ok_or_else(|| "AI configuration was not found.".to_owned())
    }

    pub fn active(&self) -> Result<Option<AiConfiguration>, String> {
        let configurations = self.read()?;
        configurations
            .active_id
            .as_deref()
            .map(|id| {
                configurations
                    .configurations
                    .iter()
                    .find(|configuration| configuration.id == id)
                    .cloned()
                    .ok_or_else(|| "The active AI configuration was not found.".to_owned())
            })
            .transpose()
    }

    pub fn create(
        &self,
        name: String,
        interaction_method: AdapterKind,
        model_id: String,
        url: String,
        system_text: String,
    ) -> Result<AiConfiguration, String> {
        validate_fields(interaction_method, &name, &model_id, &url, &system_text)?;
        let mut current = self.write()?;
        ensure_unique_name(&current.configurations, &name, None)?;
        let id = new_configuration_id()?;
        let configuration = AiConfiguration {
            id,
            name: name.trim().to_owned(),
            interaction_method,
            model_id: model_id.trim().to_owned(),
            url: normalize_url(&url)?,
            system_text: system_text.trim().to_owned(),
        };
        let mut next = current.clone();
        if next.active_id.is_none() {
            next.active_id = Some(configuration.id.clone());
        }
        next.configurations.push(configuration.clone());
        self.persist(&next)?;
        *current = next;
        Ok(configuration)
    }

    pub fn update(
        &self,
        id: &str,
        name: String,
        interaction_method: AdapterKind,
        model_id: String,
        url: String,
        system_text: String,
    ) -> Result<AiConfiguration, String> {
        validate_fields(interaction_method, &name, &model_id, &url, &system_text)?;
        let mut current = self.write()?;
        ensure_unique_name(&current.configurations, &name, Some(id))?;
        let mut next = current.clone();
        let configuration = next
            .configurations
            .iter_mut()
            .find(|configuration| configuration.id == id)
            .ok_or_else(|| "AI configuration was not found.".to_owned())?;
        configuration.name = name.trim().to_owned();
        configuration.interaction_method = interaction_method;
        configuration.model_id = model_id.trim().to_owned();
        configuration.url = normalize_url(&url)?;
        configuration.system_text = system_text.trim().to_owned();
        let updated = configuration.clone();
        self.persist(&next)?;
        *current = next;
        Ok(updated)
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut current = self.write()?;
        let mut next = current.clone();
        let original_len = next.configurations.len();
        next.configurations
            .retain(|configuration| configuration.id != id);
        if next.configurations.len() == original_len {
            return Err("AI configuration was not found.".to_owned());
        }
        let was_active = next.active_id.as_deref() == Some(id);
        if was_active {
            next.active_id = None;
        }
        self.persist(&next)?;
        *current = next;
        Ok(was_active)
    }

    pub fn set_active(&self, id: Option<&str>) -> Result<(), String> {
        let mut current = self.write()?;
        if id.is_some_and(|id| {
            !current
                .configurations
                .iter()
                .any(|configuration| configuration.id == id)
        }) {
            return Err("AI configuration was not found.".to_owned());
        }
        let mut next = current.clone();
        next.active_id = id.map(str::to_owned);
        self.persist(&next)?;
        *current = next;
        Ok(())
    }

    pub fn has_api_key(&self, id: &str) -> Result<bool, String> {
        match credential_entry(id)?.get_password() {
            Ok(_) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => Err("The operating system credential vault is unavailable.".to_owned()),
        }
    }

    pub fn get_api_key(&self, id: &str) -> Result<String, String> {
        match credential_entry(id)?.get_password() {
            Ok(key) => Ok(key),
            Err(keyring::Error::NoEntry) => {
                Err("No API key is saved for this AI configuration.".to_owned())
            }
            Err(_) => Err("The operating system credential vault is unavailable.".to_owned()),
        }
    }

    pub fn save_api_key(&self, id: &str, key: &str) -> Result<(), String> {
        if key.trim().is_empty() {
            return Err("API key cannot be empty.".to_owned());
        }
        if key.len() > MAX_API_KEY_BYTES {
            return Err("API key is too long.".to_owned());
        }
        credential_entry(id)?.set_password(key.trim()).map_err(|_| {
            "Could not save the API key to the operating system credential vault.".to_owned()
        })
    }

    pub fn forget_api_key(&self, id: &str) -> Result<(), String> {
        match credential_entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(
                "Could not remove the API key from the operating system credential vault."
                    .to_owned(),
            ),
        }
    }

    fn read(&self) -> Result<RwLockReadGuard<'_, ConfigurationFile>, String> {
        self.configurations
            .read()
            .map_err(|_| "AI configurations are unavailable.".to_owned())
    }

    fn write(&self) -> Result<RwLockWriteGuard<'_, ConfigurationFile>, String> {
        self.configurations
            .write()
            .map_err(|_| "AI configurations are unavailable.".to_owned())
    }

    fn persist(&self, configurations: &ConfigurationFile) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .ok_or_else(|| "Could not locate the AI settings directory.".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the AI settings directory: {error}"))?;
        let serialized = serde_json::to_vec_pretty(configurations)
            .map_err(|error| format!("Could not serialize AI configurations: {error}"))?;
        let temporary_path = path.with_extension("json.tmp");
        fs::write(&temporary_path, serialized)
            .map_err(|error| format!("Could not save AI configurations: {error}"))?;
        fs::rename(&temporary_path, path)
            .map_err(|error| format!("Could not save AI configurations: {error}"))
    }
}

fn validate_file(file: &ConfigurationFile) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for configuration in &file.configurations {
        if configuration.id.is_empty() || !ids.insert(configuration.id.as_str()) {
            return Err("AI configuration file contains an invalid or duplicate ID.".to_owned());
        }
        validate_fields(
            configuration.interaction_method,
            &configuration.name,
            &configuration.model_id,
            &configuration.url,
            &configuration.system_text,
        )?;
        if !names.insert(configuration.name.to_lowercase()) {
            return Err("AI configuration file contains duplicate names.".to_owned());
        }
    }
    if file
        .active_id
        .as_ref()
        .is_some_and(|id| !ids.contains(id.as_str()))
    {
        return Err("AI configuration file refers to a missing active configuration.".to_owned());
    }
    Ok(())
}

fn validate_fields(
    interaction_method: AdapterKind,
    name: &str,
    model_id: &str,
    url: &str,
    system_text: &str,
) -> Result<(), String> {
    if !is_supported_interaction_method(interaction_method) {
        return Err("Unsupported AI interaction method.".to_owned());
    }
    if name.trim().is_empty() || name.trim().len() > MAX_NAME_BYTES {
        return Err(format!(
            "Name must contain between 1 and {MAX_NAME_BYTES} bytes."
        ));
    }
    if model_id.trim().is_empty() || model_id.trim().len() > MAX_MODEL_ID_BYTES {
        return Err(format!(
            "Model ID must contain between 1 and {MAX_MODEL_ID_BYTES} bytes."
        ));
    }
    if url.trim().is_empty() || url.trim().len() > MAX_URL_BYTES {
        return Err(format!(
            "URL must contain between 1 and {MAX_URL_BYTES} bytes."
        ));
    }
    if system_text.trim().is_empty() || system_text.trim().len() > MAX_SYSTEM_TEXT_BYTES {
        return Err(format!(
            "System text must contain between 1 and {MAX_SYSTEM_TEXT_BYTES} bytes."
        ));
    }
    normalize_url(url).map(|_| ())
}

pub(crate) fn normalize_url(url: &str) -> Result<String, String> {
    let mut parsed = Url::parse(url.trim()).map_err(|error| format!("Invalid API URL: {error}"))?;
    let local_http = parsed.scheme() == "http"
        && parsed
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]"));
    if parsed.scheme() != "https" && !local_http {
        return Err("API URL must use HTTPS (HTTP is allowed only for localhost).".to_owned());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("API URL must not contain user information.".to_owned());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("API URL must not contain a query or fragment.".to_owned());
    }
    if !parsed.path().ends_with('/') {
        parsed.set_path(&format!("{}/", parsed.path()));
    }
    Ok(parsed.to_string())
}

fn ensure_unique_name(
    configurations: &[AiConfiguration],
    name: &str,
    except_id: Option<&str>,
) -> Result<(), String> {
    if configurations.iter().any(|configuration| {
        Some(configuration.id.as_str()) != except_id
            && configuration.name.eq_ignore_ascii_case(name.trim())
    }) {
        return Err("An AI configuration with that name already exists.".to_owned());
    }
    Ok(())
}

fn new_configuration_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|error| format!("Could not generate an AI configuration ID: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn credential_entry(id: &str) -> Result<Entry, String> {
    Entry::new("DFCompanion", &format!("ai-configuration-{id}"))
        .map_err(|_| "Could not access the operating system credential vault.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::adapter::AdapterKind;

    const SYSTEM_TEXT: &str = "Answer carefully.";

    fn create(repository: &ConfigurationRepository, name: &str) -> AiConfiguration {
        repository
            .create(
                name.to_owned(),
                AdapterKind::OpenAI,
                "test-model".to_owned(),
                "https://api.openai.com/v1/".to_owned(),
                SYSTEM_TEXT.to_owned(),
            )
            .unwrap()
    }

    #[test]
    fn supported_interaction_methods_have_labels() {
        assert_eq!(
            interaction_method_label(AdapterKind::OpenAI),
            Some("OpenAI Chat Completions")
        );
        assert_eq!(
            interaction_method_label(AdapterKind::OpenAIResp),
            Some("OpenAI Responses")
        );
        assert_eq!(
            interaction_method_label(AdapterKind::Anthropic),
            Some("Anthropic")
        );
        assert_eq!(interaction_method_label(AdapterKind::Gemini), None);
    }

    #[test]
    fn configuration_crud_tracks_selection_and_rename() {
        let repository = ConfigurationRepository::in_memory();
        let first = create(&repository, "Primary");
        let second = create(&repository, "Secondary");

        assert_eq!(repository.active().unwrap().unwrap().id, first.id);
        repository.set_active(Some(&second.id)).unwrap();
        let renamed = repository
            .update(
                &second.id,
                "Renamed".to_owned(),
                AdapterKind::Anthropic,
                "claude-model".to_owned(),
                "https://api.anthropic.com/".to_owned(),
                "Custom system text.".to_owned(),
            )
            .unwrap();
        assert_eq!(renamed.name, "Renamed");
        assert_eq!(renamed.interaction_method, AdapterKind::Anthropic);
        assert!(repository.delete(&second.id).unwrap());
        assert!(repository.active().unwrap().is_none());
        assert_eq!(repository.list().unwrap().0.len(), 1);
    }

    #[test]
    fn configuration_names_and_connection_data_are_validated() {
        let repository = ConfigurationRepository::in_memory();
        create(&repository, "Primary");
        let duplicate = repository.create(
            "primary".to_owned(),
            AdapterKind::Anthropic,
            "model".to_owned(),
            "https://api.anthropic.com/".to_owned(),
            SYSTEM_TEXT.to_owned(),
        );
        assert!(duplicate.unwrap_err().contains("already exists"));

        assert!(normalize_url("http://example.com/v1").is_err());
        assert!(normalize_url("https://user:pass@example.com/v1").is_err());
        assert_eq!(
            normalize_url("https://example.com/v1").unwrap(),
            "https://example.com/v1/"
        );
    }

    #[test]
    fn configurations_accept_the_previous_interaction_method_field() {
        let configuration: AiConfiguration = serde_json::from_str(
            r#"{"id":"config-id","name":"Legacy","method":"open_ai_responses","model_id":"gpt-test","url":"https://api.openai.com/v1/","system_text":"System text"}"#,
        )
        .unwrap();
        assert_eq!(configuration.interaction_method, AdapterKind::OpenAIResp);
    }

    #[test]
    fn persisted_configuration_metadata_never_contains_api_keys() {
        let path = std::env::temp_dir().join(format!(
            "dfcompanion-config-test-{}.json",
            std::process::id()
        ));
        let repository = ConfigurationRepository::load(path.clone()).unwrap();
        let configuration = create(&repository, "Primary");
        repository
            .update(
                &configuration.id,
                "Updated".to_owned(),
                AdapterKind::OpenAIResp,
                "gpt-test".to_owned(),
                "https://api.openai.com/v1/".to_owned(),
                "Updated system text.".to_owned(),
            )
            .unwrap();
        let contents = fs::read_to_string(&path).unwrap();

        assert!(contents.contains(&configuration.id));
        assert!(contents.contains("Updated"));
        assert!(contents.contains("system_text"));
        assert!(!contents.contains("api_key"));
        assert!(!contents.contains("secret"));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn deleting_an_inactive_configuration_preserves_active_selection() {
        let repository = ConfigurationRepository::in_memory();
        let first = create(&repository, "First");
        let second = create(&repository, "Second");

        assert!(!repository.delete(&second.id).unwrap());
        assert_eq!(repository.active().unwrap().unwrap().id, first.id);
    }
}
