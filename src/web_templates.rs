//! HTML rendering for the web UI. All Askama usage lives in this module so the
//! HTTP handlers in `web.rs` deal only in plain data and finished HTML strings.

use std::fmt;

use askama::Template;
use genai::adapter::AdapterKind;

use crate::{
    ai_config::{interaction_method_label, AiConfiguration},
    ai_providers::Provider,
};

#[derive(Debug)]
pub struct RenderError(String);

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<askama::Error> for RenderError {
    fn from(error: askama::Error) -> Self {
        Self(error.to_string())
    }
}

/// A configuration as shown in the list and the active-configuration selector.
#[derive(Debug)]
pub struct ConfigurationListItem {
    id: String,
    name: String,
    interaction_method: String,
    model_id: String,
    api_key_saved: bool,
    active: bool,
}

impl ConfigurationListItem {
    pub fn from_configuration(
        configuration: &AiConfiguration,
        api_key_saved: bool,
        active: bool,
    ) -> Self {
        Self {
            id: configuration.id.clone(),
            name: configuration.name.clone(),
            interaction_method: interaction_method_label(configuration.interaction_method)
                .unwrap_or("Unsupported interaction method")
                .to_owned(),
            model_id: configuration.model_id.clone(),
            api_key_saved,
            active,
        }
    }
}

/// Field values used to (re)populate the create-configuration form.
#[derive(Default)]
pub struct CreateFormValues {
    pub name: String,
    pub url: String,
    pub model_id: String,
    pub system_text: String,
}

struct InteractionMethodOption {
    value: String,
    label: String,
}

struct ProviderOption {
    name: String,
}

struct FormFieldValues {
    field_prefix: String,
    name: String,
    url: String,
    model_id: String,
    system_text: String,
}

/// An entry in the conversation and notifications list.
pub enum Notification<'a> {
    /// Text from the player or the companion; rendered as sanitized Markdown.
    Speech { speaker: &'a str, markdown: &'a str },
    /// An echo of a game event, which the page can hide.
    GameEvent(&'a str),
    /// A summary of the AI tokens a turn used, which the page can hide.
    AiUsage(&'a str),
    /// Any other notice, shown as plain text.
    Text(&'a str),
}

#[derive(Template)]
#[template(path = "notification.html")]
struct NotificationTemplate<'a> {
    class: &'a str,
    label: &'a str,
    body: &'a str,
    markdown: bool,
}

#[derive(Template)]
#[template(path = "chat-sent.html")]
struct ChatSentTemplate {
    oob: bool,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    configurations: &'a [ConfigurationListItem],
    has_active: bool,
    oob: bool,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "ai-configurations.html")]
struct ConfigurationsPageTemplate<'a> {
    configurations: &'a [ConfigurationListItem],
    has_active: bool,
    oob: bool,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "configuration-list.html")]
struct ConfigurationListTemplate<'a> {
    configurations: &'a [ConfigurationListItem],
}

#[derive(Template)]
#[template(path = "active-configuration.html")]
struct ActiveConfigurationTemplate<'a> {
    configurations: &'a [ConfigurationListItem],
    has_active: bool,
    oob: bool,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "configuration-summary.html")]
struct ConfigurationSummaryTemplate<'a> {
    configuration: &'a ConfigurationListItem,
}

#[derive(Template)]
#[template(path = "configuration-api-key-form.html")]
struct ApiKeyFormTemplate<'a> {
    configuration: &'a ConfigurationListItem,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "configuration-create-closed.html")]
struct ConfigurationCreateClosedTemplate;

#[derive(Template)]
#[template(path = "configuration-create-form.html")]
struct ConfigurationCreateFormTemplate<'a> {
    values: FormFieldValues,
    providers: Vec<ProviderOption>,
    interaction_methods: Vec<InteractionMethodOption>,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "configuration-create-success.html")]
struct ConfigurationCreateSuccessTemplate<'a> {
    configuration: &'a ConfigurationListItem,
}

pub fn render_notification(notification: &Notification) -> Result<String, RenderError> {
    let rendered = match notification {
        Notification::Speech { speaker, markdown } => {
            let body = markdown_to_safe_html(markdown);
            NotificationTemplate {
                class: "markdown",
                label: speaker,
                body: &body,
                markdown: true,
            }
            .render()?
        }
        Notification::GameEvent(text) => NotificationTemplate {
            class: "game-event",
            label: "Game event:",
            body: text,
            markdown: false,
        }
        .render()?,
        Notification::AiUsage(text) => NotificationTemplate {
            class: "ai-usage",
            label: "AI usage:",
            body: text,
            markdown: false,
        }
        .render()?,
        Notification::Text(text) => NotificationTemplate {
            class: "",
            label: "",
            body: text,
            markdown: false,
        }
        .render()?,
    };
    Ok(rendered.trim().to_owned())
}

/// Companion and player text is untrusted Markdown (model output, game
/// names), so it is always sanitized: links open safely, and images and
/// styling are dropped.
fn markdown_to_safe_html(markdown: &str) -> String {
    use pulldown_cmark::{html, Event, Options, Parser};

    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(markdown, options).map(|event| match event {
        Event::SoftBreak => Event::HardBreak,
        event => event,
    });
    let mut unsafe_html = String::new();
    html::push_html(&mut unsafe_html, parser);

    ammonia::Builder::default()
        .rm_tags(&["img", "style"])
        .link_rel(Some("noopener noreferrer"))
        .set_tag_attribute_value("a", "target", "_blank")
        .clean(&unsafe_html)
        .to_string()
}

/// The chat status line, plus an out-of-band replacement that empties the
/// message box (setting `value` is the only way to clear what was typed).
pub fn render_chat_sent() -> Result<String, RenderError> {
    Ok(ChatSentTemplate { oob: true }.render()?)
}

/// Renders the main page with the configuration selector already in place.
pub fn render_index(configurations: &[ConfigurationListItem]) -> Result<String, RenderError> {
    Ok(IndexTemplate {
        configurations,
        has_active: has_active(configurations),
        oob: false,
        error: "",
    }
    .render()?)
}

/// Renders the configuration management page with the selector and the list
/// of configurations already in place.
pub fn render_configurations_page(
    configurations: &[ConfigurationListItem],
) -> Result<String, RenderError> {
    Ok(ConfigurationsPageTemplate {
        configurations,
        has_active: has_active(configurations),
        oob: false,
        error: "",
    }
    .render()?)
}

fn has_active(configurations: &[ConfigurationListItem]) -> bool {
    configurations
        .iter()
        .any(|configuration| configuration.active)
}

pub fn render_configuration_list(
    configurations: &[ConfigurationListItem],
) -> Result<String, RenderError> {
    Ok(ConfigurationListTemplate { configurations }.render()?)
}

/// Renders the active-configuration selector. With `oob` set, the fragment is
/// marked to replace the selector already on the page.
pub fn render_active_configuration(
    configurations: &[ConfigurationListItem],
    oob: bool,
    error: &str,
) -> Result<String, RenderError> {
    Ok(ActiveConfigurationTemplate {
        configurations,
        has_active: has_active(configurations),
        oob,
        error,
    }
    .render()?)
}

/// Renders one list item: its name and an add or forget API key button.
pub fn render_configuration_summary(
    configuration: &ConfigurationListItem,
) -> Result<String, RenderError> {
    Ok(ConfigurationSummaryTemplate { configuration }.render()?)
}

/// Renders the list item as a form for entering the configuration's API key.
pub fn render_api_key_form(
    configuration: &ConfigurationListItem,
    error: &str,
) -> Result<String, RenderError> {
    Ok(ApiKeyFormTemplate {
        configuration,
        error,
    }
    .render()?)
}

pub fn render_create_closed() -> Result<String, RenderError> {
    Ok(ConfigurationCreateClosedTemplate.render()?)
}

pub fn render_create_form(
    values: CreateFormValues,
    providers: &[Provider],
    error: &str,
) -> Result<String, RenderError> {
    Ok(ConfigurationCreateFormTemplate {
        values: FormFieldValues {
            field_prefix: "new".to_owned(),
            name: values.name,
            url: values.url,
            model_id: values.model_id,
            system_text: values.system_text,
        },
        providers: providers
            .iter()
            .map(|provider| ProviderOption {
                name: provider.name.clone(),
            })
            .collect(),
        interaction_methods: AdapterKind::all()
            .iter()
            .filter_map(|kind| {
                interaction_method_label(*kind).map(|label| InteractionMethodOption {
                    value: kind.as_str().to_owned(),
                    label: label.to_owned(),
                })
            })
            .collect(),
        error,
    }
    .render()?)
}

/// Renders a newly created configuration's list item followed by the closed
/// "Create new configuration" row.
pub fn render_created_configuration(
    configuration: &ConfigurationListItem,
) -> Result<String, RenderError> {
    Ok(ConfigurationCreateSuccessTemplate { configuration }.render()?)
}

#[cfg(test)]
mod test {
    use super::*;

    fn item(id: &str, name: &str, key_saved: bool, active: bool) -> ConfigurationListItem {
        ConfigurationListItem {
            id: id.to_owned(),
            name: name.to_owned(),
            interaction_method: "Anthropic".to_owned(),
            model_id: "test-model".to_owned(),
            api_key_saved: key_saved,
            active,
        }
    }

    #[test]
    fn speech_is_rendered_as_markdown_with_a_label() {
        let rendered = render_notification(&Notification::Speech {
            speaker: "Companion:",
            markdown: "**Hello**\nthere",
        })
        .expect("notification should render");

        assert!(rendered.starts_with("<li class=\"markdown\"><strong>Companion: </strong>"));
        assert!(rendered.contains("<strong>Hello</strong><br>"));
        assert!(!rendered.contains("\n</li>"));
    }

    #[test]
    fn speech_markdown_is_sanitized() {
        let rendered = render_notification(&Notification::Speech {
            speaker: "Companion:",
            markdown: "<script>alert(1)</script>\n\n![x](http://e/x.png) [a](https://example.com) <img src=x onerror=alert(1)>[b](javascript:alert(1))",
        })
        .expect("notification should render");

        assert!(!rendered.contains("<script"));
        assert!(!rendered.contains("<img"));
        assert!(!rendered.contains("href=\"javascript:"));
        assert!(rendered.contains("href=\"https://example.com\""));
        assert!(rendered.contains("target=\"_blank\""));
        assert!(rendered.contains("noopener noreferrer"));
    }

    #[test]
    fn chat_sent_clears_the_message_box_out_of_band() {
        let rendered = render_chat_sent().expect("fragment should render");

        assert!(rendered.contains("Message queued."));
        assert!(rendered.contains("id=\"message\""));
        assert!(rendered.contains("hx-swap-oob=\"true\""));
    }

    #[test]
    fn pages_include_their_fragments_without_load_requests() {
        let configurations = [ConfigurationListItem::from_configuration(
            &AiConfiguration {
                id: "one".to_owned(),
                name: "First".to_owned(),
                interaction_method: AdapterKind::OpenAI,
                model_id: "m".to_owned(),
                url: "https://api.openai.com/v1/".to_owned(),
                system_text: "s".to_owned(),
            },
            true,
            true,
        )];

        let index = render_index(&configurations).expect("index should render");
        assert!(index.contains("id=\"active-configuration\""));
        assert!(index.contains("value=\"one\" checked"));
        assert!(index.contains("htmx-config"));
        assert!(index.contains("name=\"message\""));
        assert!(!index.contains("hx-trigger=\"load\"\n           hx-swap"));
        assert!(!index.contains("hx-swap-oob"));

        let page = render_configurations_page(&configurations).expect("page should render");
        assert!(page.contains("id=\"configuration-list\""));
        assert!(page.contains("First"));
        assert!(!page.contains("Loading configurations"));
    }

    #[test]
    fn ai_usage_notifications_have_their_own_class() {
        let usage = render_notification(&Notification::AiUsage("This turn: 1 API request"))
            .expect("notification should render");

        assert_eq!(
            usage,
            "<li class=\"ai-usage\"><strong>AI usage: </strong>This turn: 1 API request</li>"
        );
    }

    #[test]
    fn plain_and_game_event_notifications_are_escaped_text() {
        let game = render_notification(&Notification::GameEvent("<b>x</b>"))
            .expect("notification should render");
        let text =
            render_notification(&Notification::Text("a < b")).expect("notification should render");

        assert!(game.starts_with("<li class=\"game-event\">"));
        assert!(game.contains("Game event: "));
        assert!(!game.contains("<b>"));
        assert_eq!(text, "<li>a &#60; b</li>");
    }

    #[test]
    fn configuration_list_renders_empty_state() {
        let rendered = render_configuration_list(&[]).expect("list should render");

        assert!(rendered.contains("id=\"new-configuration\""));
        assert!(rendered.contains("Create new configuration"));
        assert!(!rendered.contains("No configurations yet."));
    }

    #[test]
    fn configuration_list_renders_and_escapes_configuration() {
        let mut configuration = item("test-id", "<script>bad</script>", true, true);
        configuration.interaction_method = "OpenAI Responses".to_owned();
        configuration.model_id = "deepseek-flash".to_owned();
        let rendered = render_configuration_list(&[configuration]).expect("list should render");

        assert!(rendered.contains("&#60;script&#62;bad&#60;/script&#62;"));
        assert!(!rendered.contains("<script>bad</script>"));
        assert!(rendered.contains("OpenAI Responses"));
        assert!(rendered.contains("deepseek-flash"));
        assert!(!rendered.contains(">Select</button>"));
        assert!(rendered.contains("id=\"configuration-test-id\""));
        assert!(rendered.contains("id=\"new-configuration\""));
    }

    #[test]
    fn configuration_list_renders_forget_key_for_saved_key() {
        let rendered = render_configuration_list(&[item("configuration-id", "Test", true, true)])
            .expect("list should render");

        assert!(
            rendered.contains("hx-delete=\"/fragments/ai-configurations/configuration-id/key\"")
        );
        assert!(rendered.contains("hx-confirm="));
        assert!(rendered.contains("Forget API key"));
        assert!(!rendered.contains("Add API key"));
    }

    #[test]
    fn configuration_list_offers_add_key_when_key_is_missing() {
        let rendered = render_configuration_list(&[item("configuration-id", "Test", false, true)])
            .expect("list should render");

        assert!(!rendered.contains("Forget API key"));
        assert!(rendered.contains("hx-get=\"/fragments/ai-configurations/configuration-id/key\""));
        assert!(rendered.contains("Add API key"));
    }

    #[test]
    fn api_key_form_submits_a_password_field_and_shows_error() {
        let rendered = render_api_key_form(&item("configuration-id", "Test", false, true), "Nope")
            .expect("form should render");

        assert!(rendered.contains("hx-put=\"/fragments/ai-configurations/configuration-id/key\""));
        assert!(rendered.contains("name=\"api_key\""));
        assert!(rendered.contains("type=\"password\""));
        assert!(rendered.contains("hx-get=\"/fragments/ai-configurations/configuration-id\""));
        assert!(rendered.contains("Nope"));
    }

    #[test]
    fn create_form_has_no_api_key_field() {
        let rendered =
            render_create_form(CreateFormValues::default(), &[], "").expect("form should render");

        assert!(!rendered.contains("name=\"api_key\""));
    }

    #[test]
    fn active_configuration_marks_active_and_flags_missing_key() {
        let configurations = [
            item("id-1", "First", false, false),
            item("id-2", "Second", true, true),
        ];

        let rendered =
            render_active_configuration(&configurations, true, "").expect("selector should render");

        assert!(rendered.contains("hx-swap-oob=\"true\""));
        assert!(rendered.contains("hx-put=\"/fragments/ai-configurations/active\""));
        assert!(rendered.contains("value=\"id-1\">"));
        assert!(rendered.contains("value=\"id-2\" checked"));
        assert!(rendered.contains("(API key needed)"));

        let plain = render_active_configuration(&configurations, false, "Oops")
            .expect("selector should render");
        assert!(!plain.contains("hx-swap-oob"));
        assert!(plain.contains("Oops"));
    }

    #[test]
    fn create_form_prefills_values_and_shows_error() {
        let values = CreateFormValues {
            name: "My config".to_owned(),
            ..Default::default()
        };
        let rendered = render_create_form(values, &[], "Bad key").expect("form should render");

        assert!(rendered.contains("value=\"My config\""));
        assert!(rendered.contains("Bad key"));
    }

    #[test]
    fn created_configuration_includes_closed_create_row() {
        let rendered = render_created_configuration(&item("new-id", "New", false, false))
            .expect("success should render");

        assert!(rendered.contains("id=\"configuration-new-id\""));
        assert!(rendered.contains("Create new configuration"));
    }
}
