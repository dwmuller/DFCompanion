use std::convert::Infallible;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        Html, IntoResponse, Response, Sse,
    },
    routing::{get, post, put},
    Router,
};
use futures_util::StreamExt as _;
use genai::adapter::AdapterKind;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    ai_config::{AiConfiguration, ConfigurationRepository},
    ai_providers::built_in_providers,
    web_templates::{
        self as templates, ConfigurationListItem, CreateFormValues, Notification, RenderError,
    },
    AppState, GameEvent, QueueItem, GAME_EVENT_PREFIX,
};

const HTTP_ORIGIN: &str = "http://127.0.0.1:3000";
const HTTP_HOST: &str = "127.0.0.1:3000";

#[derive(Deserialize)]
struct ChatForm {
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigurationForm {
    name: String,
    #[serde(
        alias = "method",
        deserialize_with = "crate::ai_config::deserialize_interaction_method"
    )]
    interaction_method: AdapterKind,
    model_id: String,
    url: String,
    system_text: String,
    api_key: Option<String>,
}

#[derive(Serialize)]
struct ConfigurationSummary {
    #[serde(flatten)]
    configuration: AiConfiguration,
    api_key_saved: bool,
}

#[derive(Serialize)]
struct ConfigurationListResponse {
    configurations: Vec<ConfigurationSummary>,
    active_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiKeyForm {
    api_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectConfigurationForm {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginEvent {
    id: Option<i64>,
    date: String,
    #[serde(rename = "type")]
    report_type: Option<String>,
    category: String,
    text: String,
}

#[derive(Serialize)]
struct PluginEventResult {
    status: &'static str,
    message: String,
}

/// Logs every HTTP request and response to stdout. Enabled by `--trace-http`.
pub fn with_request_tracing(router: Router) -> Router {
    use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
    use tracing::Level;

    tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .compact()
        .init();
    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_request(DefaultOnRequest::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO)),
    )
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ai-configurations", get(configurations_page))
        .route("/chat", post(chat))
        .route("/retry", post(retry))
        .route(
            "/fragments/ai-configurations",
            get(configuration_list_fragment).post(create_configuration_fragment),
        )
        .route(
            "/fragments/ai-configurations/new",
            get(new_configuration_form_fragment),
        )
        .route(
            "/fragments/ai-configurations/new/closed",
            get(closed_configuration_form_fragment),
        )
        .route(
            "/fragments/ai-configurations/active",
            get(active_configuration_fragment)
                .put(select_configuration_fragment)
                .layer(DefaultBodyLimit::max(8_192)),
        )
        .route(
            "/fragments/ai-configurations/:id",
            get(configuration_summary_fragment),
        )
        .route(
            "/fragments/ai-configurations/:id/key",
            get(api_key_form_fragment)
                .put(save_api_key_fragment)
                .delete(forget_api_key_fragment)
                .layer(DefaultBodyLimit::max(8_192)),
        )
        .route("/api/ai-providers", get(list_providers))
        .route(
            "/api/ai-configurations/:id",
            put(update_configuration)
                .delete(delete_configuration)
                .layer(DefaultBodyLimit::max(32_768)),
        )
        .route(
            "/api/plugin/events",
            post(plugin_event).layer(DefaultBodyLimit::max(16_384)),
        )
        .route("/events", get(events))
        .route("/api/automatic-turns", put(set_automatic_turns))
        .route("/assets/app.js", get(app_script))
        .route("/assets/hx-sse.min.js", get(htmx_sse_script))
        .route("/assets/htmx.min.js", get(htmx_script))
        .route("/assets/ai-configurations.js", get(configuration_script))
        .route("/assets/style.css", get(stylesheet))
        .layer(middleware::from_fn(validate_local_request))
        .with_state(state)
}

async fn plugin_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(event): axum::Json<PluginEvent>,
) -> impl IntoResponse {
    if headers.contains_key(header::ORIGIN) {
        return plugin_event_response(
            StatusCode::FORBIDDEN,
            "rejected",
            "Browser-originated requests are not accepted.",
        );
    }

    let event = GameEvent {
        id: event.id,
        date: event.date,
        report_type: event.report_type,
        category: event.category.trim().to_owned(),
        text: event.text.trim().to_owned(),
    };
    if let Err(message) = event.validate() {
        let status_code = if event.text.len() > 8_000 {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        };
        return plugin_event_response(status_code, "rejected", &message);
    }

    match state.queue_tx.try_send(QueueItem::GameEvent(event)) {
        Ok(()) => plugin_event_response(StatusCode::OK, "accepted", "Event accepted."),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            let message = format!(
                "The event queue is full (capacity {}). The event was not accepted; pause the game.",
                crate::EVENT_QUEUE_CAPACITY
            );
            state.notify(message.clone());
            plugin_event_response(StatusCode::TOO_MANY_REQUESTS, "queue_full", &message)
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => plugin_event_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "The Companion queue is unavailable.",
        ),
    }
}

fn plugin_event_response(status_code: StatusCode, status: &'static str, message: &str) -> Response {
    (
        status_code,
        axum::Json(PluginEventResult {
            status,
            message: message.to_owned(),
        }),
    )
        .into_response()
}

async fn index() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(include_str!("../static/index.html")),
    )
}

async fn configurations_page() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(include_str!("../static/ai-configurations.html")),
    )
}

async fn list_configurations(State(state): State<AppState>) -> Response {
    match configuration_list(&state.ai.configurations) {
        Ok(response) => {
            ([(header::CACHE_CONTROL, "no-store")], axum::Json(response)).into_response()
        }
        Err(response) => response,
    }
}

fn fragment_response(
    status: StatusCode,
    rendered: Result<String, RenderError>,
    what: &str,
) -> Response {
    match rendered {
        Ok(html) => (status, [(header::CACHE_CONTROL, "no-store")], Html(html)).into_response(),
        Err(error) => {
            eprintln!("Could not render {what}: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not render {what}."),
            )
                .into_response()
        }
    }
}

async fn configuration_list_fragment(State(state): State<AppState>) -> Response {
    let items = match configuration_list(&state.ai.configurations) {
        Ok(response) => list_items(&response),
        Err(response) => return response,
    };

    let rendered =
        templates::render_configuration_list(&items).and_then(|list| with_selector(&state, list));
    fragment_response(StatusCode::OK, rendered, "AI configurations")
}

async fn new_configuration_form_fragment(State(_state): State<AppState>) -> Response {
    create_form_response(StatusCode::OK, CreateFormValues::default(), "")
}

async fn closed_configuration_form_fragment(State(_state): State<AppState>) -> Response {
    fragment_response(
        StatusCode::OK,
        templates::render_create_closed(),
        "closed configuration form",
    )
}

fn create_form_response(status: StatusCode, values: CreateFormValues, error: &str) -> Response {
    let providers = match built_in_providers() {
        Ok(providers) => providers,
        Err(error) => {
            eprintln!("Could not retrieve built-in providers: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not retrieve built-in providers.",
            )
                .into_response();
        }
    };
    fragment_response(
        status,
        templates::render_create_form(values, &providers, error),
        "new configuration form",
    )
}

async fn create_configuration_fragment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    let configuration = match state.ai.configurations.create(
        form.name,
        form.interaction_method,
        form.model_id,
        form.url,
        form.system_text,
    ) {
        Ok(configuration) => configuration,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    let item = match configuration_item(&state, &configuration) {
        Ok(item) => item,
        Err(response) => return response,
    };
    let rendered = templates::render_created_configuration(&item)
        .and_then(|created| with_selector(&state, created));
    fragment_response(StatusCode::OK, rendered, "created configuration")
}

async fn list_providers() -> Response {
    match crate::ai_providers::built_in_providers() {
        Ok(providers) => {
            ([(header::CACHE_CONTROL, "no-store")], axum::Json(providers)).into_response()
        }
        Err(error) => {
            eprintln!("{error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not load built-in AI providers.",
            )
                .into_response()
        }
    }
}

async fn update_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(form): axum::Json<ConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }

    if let Err(error) = state.ai.configurations.get(&id) {
        return (StatusCode::NOT_FOUND, error).into_response();
    }

    if form
        .api_key
        .as_ref()
        .is_some_and(|key| key.len() > crate::ai_config::MAX_API_KEY_BYTES)
    {
        return (StatusCode::PAYLOAD_TOO_LARGE, "API key is too long.").into_response();
    }
    let new_key = form.api_key.filter(|key| !key.trim().is_empty());
    let previous_key = if new_key.is_some() {
        match state.ai.configurations.has_api_key(&id) {
            Ok(true) => match state.ai.configurations.get_api_key(&id) {
                Ok(key) => Some(key),
                Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
            },
            Ok(false) => None,
            Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
        }
    } else {
        None
    };
    if let Some(key) = &new_key {
        if let Err(error) = state.ai.configurations.save_api_key(&id, key) {
            return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
        }
    }

    let updated = state.ai.configurations.update(
        &id,
        form.name,
        form.interaction_method,
        form.model_id,
        form.url,
        form.system_text,
    );
    if let Err(error) = updated {
        if new_key.is_some() {
            let rollback = match previous_key {
                Some(previous_key) => state.ai.configurations.save_api_key(&id, &previous_key),
                None => state.ai.configurations.forget_api_key(&id),
            };
            if let Err(rollback_error) = rollback {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{error} API key rollback also failed: {rollback_error}"),
                )
                    .into_response();
            }
        }
        return (StatusCode::BAD_REQUEST, error).into_response();
    }
    list_configurations(State(state)).await
}

async fn active_configuration_fragment(State(state): State<AppState>) -> Response {
    active_configuration_response(&state, StatusCode::OK, String::new())
}

async fn select_configuration_fragment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SelectConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }

    let id = Some(form.id.as_str()).filter(|id| !id.is_empty());
    if let Err(error) = state.ai.configurations.set_active(id) {
        return active_configuration_response(&state, StatusCode::BAD_REQUEST, error);
    }
    if id.is_some() {
        let _ = state.queue_tx.try_send(QueueItem::Orient);
    }

    active_configuration_response(&state, StatusCode::OK, String::new())
}

fn active_configuration_response(state: &AppState, status: StatusCode, error: String) -> Response {
    let items = match configuration_list(&state.ai.configurations) {
        Ok(response) => list_items(&response),
        Err(response) => return response,
    };
    fragment_response(
        status,
        templates::render_active_configuration(&items, false, &error),
        "active AI configuration",
    )
}

/// Looks up a configuration and builds its list item, or the error response.
fn configuration_item(
    state: &AppState,
    configuration: &AiConfiguration,
) -> Result<ConfigurationListItem, Response> {
    let has_api_key = state
        .ai
        .configurations
        .has_api_key(&configuration.id)
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error).into_response())?;
    let active = state
        .ai
        .configurations
        .active()
        .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error).into_response())?
        .is_some_and(|active| active.id == configuration.id);
    Ok(ConfigurationListItem::from_configuration(
        configuration,
        has_api_key,
        active,
    ))
}

fn existing_configuration_item(
    state: &AppState,
    id: &str,
) -> Result<ConfigurationListItem, Response> {
    let configuration = state
        .ai
        .configurations
        .get(id)
        .map_err(|error| (StatusCode::NOT_FOUND, error).into_response())?;
    configuration_item(state, &configuration)
}

/// Appends the active-configuration selector as an out-of-band swap, because
/// it shows which configurations still need an API key.
fn with_selector(state: &AppState, html: String) -> Result<String, RenderError> {
    let items = match configuration_list(&state.ai.configurations) {
        Ok(response) => list_items(&response),
        Err(_) => return Ok(html),
    };
    templates::render_active_configuration(&items, true, "").map(|selector| html + &selector)
}

fn summary_response(state: &AppState, item: &ConfigurationListItem) -> Response {
    let rendered = templates::render_configuration_summary(item)
        .and_then(|summary| with_selector(state, summary));
    fragment_response(StatusCode::OK, rendered, "AI configuration")
}

async fn configuration_summary_fragment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match existing_configuration_item(&state, &id) {
        Ok(item) => fragment_response(
            StatusCode::OK,
            templates::render_configuration_summary(&item),
            "AI configuration",
        ),
        Err(response) => response,
    }
}

async fn api_key_form_fragment(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match existing_configuration_item(&state, &id) {
        Ok(item) => fragment_response(
            StatusCode::OK,
            templates::render_api_key_form(&item, ""),
            "API key form",
        ),
        Err(response) => response,
    }
}

async fn save_api_key_fragment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<ApiKeyForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    let item = match existing_configuration_item(&state, &id) {
        Ok(item) => item,
        Err(response) => return response,
    };

    let rejection = if form.api_key.trim().is_empty() {
        Some((StatusCode::BAD_REQUEST, "Enter an API key."))
    } else if form.api_key.len() > crate::ai_config::MAX_API_KEY_BYTES {
        Some((StatusCode::PAYLOAD_TOO_LARGE, "API key is too long."))
    } else {
        None
    };
    if let Some((status, message)) = rejection {
        return fragment_response(
            status,
            templates::render_api_key_form(&item, message),
            "API key form",
        );
    }

    if let Err(error) = state.ai.configurations.save_api_key(&id, &form.api_key) {
        return fragment_response(
            StatusCode::SERVICE_UNAVAILABLE,
            templates::render_api_key_form(&item, &error),
            "API key form",
        );
    }
    match existing_configuration_item(&state, &id) {
        Ok(item) => summary_response(&state, &item),
        Err(response) => response,
    }
}

async fn forget_api_key_fragment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    if let Err(response) = existing_configuration_item(&state, &id) {
        return response;
    }
    if let Err(error) = state.ai.configurations.forget_api_key(&id) {
        return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
    }
    match existing_configuration_item(&state, &id) {
        Ok(item) => summary_response(&state, &item),
        Err(response) => response,
    }
}

async fn delete_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    if let Err(error) = state.ai.configurations.get(&id) {
        return (StatusCode::NOT_FOUND, error).into_response();
    }
    let previous_key = match state.ai.configurations.has_api_key(&id) {
        Ok(true) => match state.ai.configurations.get_api_key(&id) {
            Ok(key) => Some(key),
            Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
        },
        Ok(false) => None,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    if let Err(error) = state.ai.configurations.forget_api_key(&id) {
        return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
    }
    if let Err(error) = state.ai.configurations.delete(&id) {
        let rollback = previous_key.map_or(Ok(()), |key| {
            state.ai.configurations.save_api_key(&id, &key)
        });
        if let Err(rollback_error) = rollback {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{error} API key rollback also failed: {rollback_error}"),
            )
                .into_response();
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response();
    }
    list_configurations(State(state)).await
}

fn configuration_list(
    repository: &ConfigurationRepository,
) -> Result<ConfigurationListResponse, Response> {
    let (configurations, active_id) = repository
        .list()
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error).into_response())?;
    let configurations = configurations
        .into_iter()
        .map(|configuration| {
            let api_key_saved = repository
                .has_api_key(&configuration.id)
                .map_err(|error| (StatusCode::SERVICE_UNAVAILABLE, error).into_response())?;
            Ok(ConfigurationSummary {
                configuration,
                api_key_saved,
            })
        })
        .collect::<Result<Vec<_>, Response>>()?;
    Ok(ConfigurationListResponse {
        configurations,
        active_id,
    })
}

fn list_items(response: &ConfigurationListResponse) -> Vec<ConfigurationListItem> {
    response
        .configurations
        .iter()
        .map(|summary| {
            ConfigurationListItem::from_configuration(
                &summary.configuration,
                summary.api_key_saved,
                response.active_id.as_ref() == Some(&summary.configuration.id),
            )
        })
        .collect()
}

async fn configuration_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/ai-configurations.js"),
    )
}

async fn app_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/app.js"),
    )
}

async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/htmx.min.js"),
    )
}

async fn htmx_sse_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/hx-sse.min.js"),
    )
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

/// Turns a recorded notification into the HTML list item the page appends.
/// `live` is false while replaying history, so replays never beep.
fn notification_events(id: u64, message: &str, live: bool) -> Vec<Event> {
    let (notification, unsolicited) = if let Some(text) = message.strip_prefix(GAME_EVENT_PREFIX) {
        (Notification::GameEvent(text), false)
    } else if let Some(text) = message.strip_prefix(crate::AI_USAGE_PREFIX) {
        (Notification::AiUsage(text), false)
    } else if let Some(text) = message.strip_prefix(crate::UNSOLICITED_PREFIX) {
        (
            Notification::Speech {
                speaker: "Companion:",
                markdown: text,
            },
            true,
        )
    } else if let Some(text) = message.strip_prefix("You: ") {
        (
            Notification::Speech {
                speaker: "You:",
                markdown: text,
            },
            false,
        )
    } else if let Some(text) = message.strip_prefix("Companion: ") {
        (
            Notification::Speech {
                speaker: "Companion:",
                markdown: text,
            },
            false,
        )
    } else {
        (Notification::Text(message), false)
    };

    let html = match templates::render_notification(&notification) {
        Ok(html) => html,
        Err(error) => {
            eprintln!("Could not render notification: {error}");
            return Vec::new();
        }
    };
    // Unnamed events are swapped into the page; named ones become DOM events.
    let mut events = vec![Event::default().id(id.to_string()).data(html)];
    if unsolicited && live {
        events.push(Event::default().event("beep").data(""));
    }
    events
}

/// Checkboxes are only submitted when checked, so presence means enabled.
#[derive(Deserialize)]
struct AutomaticTurnsForm {
    event_turns: Option<String>,
    periodic_turns: Option<String>,
}

async fn set_automatic_turns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AutomaticTurnsForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    state.event_turns.store(
        form.event_turns.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    state.periodic_turns.store(
        form.periodic_turns.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    // A reconnecting browser says where it left off, so it isn't sent
    // messages it already has.
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let (backlog, live) = state.history.subscribe(last_event_id);
    let backlog = tokio_stream::iter(backlog.into_iter().flat_map(|(id, message)| {
        notification_events(id, &message, false)
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>()
    }));
    let live = BroadcastStream::new(live).flat_map(|notification| {
        let events = match notification {
            Ok((id, message)) => notification_events(id, &message, true),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                vec![Event::default().data(
                    templates::render_notification(&Notification::Text(&format!(
                        "{count} UI notifications were missed while the browser was disconnected."
                    )))
                    .unwrap_or_default(),
                )]
            }
        };
        tokio_stream::iter(events.into_iter().map(Ok))
    });

    Sse::new(backlog.chain(live)).keep_alive(KeepAlive::default())
}

async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ChatForm>,
) -> impl IntoResponse {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }

    let message = form.message.trim();
    if message.is_empty() {
        return (StatusCode::BAD_REQUEST, "Enter a message.").into_response();
    }
    if message.len() > 8_000 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "Message exceeds the 8000-byte limit.",
        )
            .into_response();
    }

    match state
        .queue_tx
        .try_send(QueueItem::UserInput(message.to_owned()))
    {
        Ok(()) => {
            state.notify(format!("You: {message}"));
            fragment_response(StatusCode::OK, templates::render_chat_sent(), "chat status")
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            let warning = format!(
                "The event queue is full (capacity {}). New input was not accepted.",
                crate::EVENT_QUEUE_CAPACITY
            );
            state.notify(warning.clone());
            (StatusCode::SERVICE_UNAVAILABLE, warning).into_response()
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            state.notify("The Companion queue is unavailable.");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "The Companion queue is unavailable.",
            )
                .into_response()
        }
    }
}

async fn retry(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    match state.queue_tx.try_send(QueueItem::AiRetry) {
        Ok(()) => (StatusCode::OK, "Retry queued.").into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The event queue is full. Wait briefly, then retry.",
        )
            .into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The Companion queue is unavailable.",
        )
            .into_response(),
    }
}

async fn validate_local_request(request: Request<Body>, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host != Some(HTTP_HOST) {
        return (StatusCode::FORBIDDEN, "Host not allowed.").into_response();
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn validate_local_origin(headers: &HeaderMap) -> Result<(), StatusCode> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;

    if origin != HTTP_ORIGIN {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    fn summary(id: &str, name: &str) -> ConfigurationSummary {
        ConfigurationSummary {
            configuration: AiConfiguration {
                id: id.to_owned(),
                name: name.to_owned(),
                interaction_method: AdapterKind::OpenAIResp,
                model_id: "test-model".to_owned(),
                url: "https://example.com/".to_owned(),
                system_text: "Test system text".to_owned(),
            },
            api_key_saved: true,
        }
    }

    #[test]
    fn list_items_maps_summaries_and_marks_the_active_configuration() {
        let response = ConfigurationListResponse {
            configurations: vec![
                summary("test-id-1", "Test configuration 1"),
                summary("test-id-2", "Test configuration 2"),
            ],
            active_id: Some("test-id-1".to_owned()),
        };

        let items = list_items(&response);
        let list = templates::render_configuration_list(&items).expect("list should render");
        let selector = templates::render_active_configuration(&items, false, "")
            .expect("selector should render");

        assert!(list.contains("Test configuration 2"));
        assert!(list.contains("OpenAI Responses"));
        assert!(selector.contains("value=\"test-id-1\" selected"));
        assert!(!selector.contains("value=\"test-id-2\" selected"));
    }
}
