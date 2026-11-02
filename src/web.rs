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
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    ai_config::{
        supported_interaction_method_from_name, supported_interaction_method_name, AiConfiguration,
        ConfigurationRepository,
    },
    ai_providers::built_in_providers,
    web_templates::{
        self as templates, render_configuration_form_error, ConfigurationListItem, Notification,
        RenderError,
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
struct ConfigurationForm {
    name: String,
    interaction_method: String,
    model_id: String,
    url: String,
    system_text: String,
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
            get(configuration_list_fragment),
        )
        .route(
            "/fragments/ai-configurations/new",
            get(new_configuration_form_fragment).post(create_configuration_fragment),
        )
        .route(
            "/fragments/ai-configurations/new/closed",
            get(closed_new_configuration_form_fragment),
        )
        .route(
            "/fragments/ai-configurations/:id",
            get(edit_configuration_form_fragment)
                .put(update_configuration_fragment)
                .delete(delete_configuration),
        )
        .route(
            "/fragments/ai-configurations/:id/closed",
            get(closed_configuration_summary_fragment),
        )
        .route(
            "/fragments/ai-configurations/:id/key",
            get(api_key_form_fragment)
                .put(save_api_key_fragment)
                .delete(forget_api_key_fragment)
                .layer(DefaultBodyLimit::max(8_192)),
        )
        .route(
            "/fragments/ai-configurations/active",
            get(active_configuration_fragment)
                .put(select_configuration_fragment)
                .layer(DefaultBodyLimit::max(8_192)),
        )
        .route("/api/ai-providers", get(list_providers))
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

/// Handles POST requests for events from DFHack plugin.
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

/// Handles GET requests for the index page.
async fn index(State(state): State<AppState>) -> Response {
    match current_items(&state) {
        Ok(items) => {
            let active_id = match get_active_id(&state) {
                Ok(id) => id,
                Err(response) => return response,
            };
            fragment_response(
                StatusCode::OK,
                templates::render_index(&items, active_id.as_deref()),
                "page",
            )
        }
        Err(response) => response,
    }
}

/// Handles GET requests for the AI configurations page.
async fn configurations_page(State(state): State<AppState>) -> Response {
    match current_items(&state) {
        Ok(items) => {
            let active_id = match get_active_id(&state) {
                Ok(id) => id,
                Err(response) => return response,
            };
            fragment_response(
                StatusCode::OK,
                templates::render_configurations_page(&items, active_id.as_deref()),
                "page",
            )
        }
        Err(response) => response,
    }
}

fn current_items(state: &AppState) -> Result<Vec<ConfigurationListItem>, Response> {
    configuration_list(&state.ai.configurations).map(|response| list_items(&response))
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

/// Handles GET requests for the AI configuration list fragment
async fn configuration_list_fragment(State(state): State<AppState>) -> Response {
    let items = match configuration_list(&state.ai.configurations) {
        Ok(response) => list_items(&response),
        Err(response) => return response,
    };

    let rendered =
        templates::render_configuration_list(&items).and_then(|list| with_selector(&state, list));
    fragment_response(StatusCode::OK, rendered, "AI configurations")
}

/// Handles GET requests for the new AI configuration form fragment
async fn new_configuration_form_fragment(State(_state): State<AppState>) -> Response {
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
        StatusCode::OK,
        templates::render_empty_create_form(&providers, ""),
        "new configuration form",
    )
}

/// Handles GET requests for the edit AI configuration form fragment
async fn edit_configuration_form_fragment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let config = match state.ai.configurations.get(&id) {
        Ok(config) => config,
        Err(response) => return (StatusCode::NOT_FOUND, response).into_response(),
    };
    let method_name = match supported_interaction_method_name(config.interaction_method) {
        Some(name) => name.to_owned(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Unsupported interaction method stored",
            )
                .into_response()
        }
    };
    fragment_response(
        StatusCode::OK,
        templates::render_edit_form(
            &id,
            &config.name,
            &method_name,
            &config.url,
            &config.model_id,
            &config.system_text,
            "",
        ),
        "edit configuration form",
    )
}

/// Handles GET requests for the closed AI configuration form fragment
async fn closed_new_configuration_form_fragment(State(_state): State<AppState>) -> Response {
    fragment_response(
        StatusCode::OK,
        templates::render_create_closed(),
        "closed configuration form",
    )
}

///
/// POST handler for creating a new AI configuration
///
async fn create_configuration_fragment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    let method = match supported_interaction_method_from_name(&form.interaction_method) {
        Some(method) => method,
        None => {
            return configuration_form_error_response(
                StatusCode::BAD_REQUEST,
                "new",
                "Invalid interaction method",
            );
        }
    };
    let configuration = match state.ai.configurations.create(
        &form.name,
        method,
        &form.model_id,
        &form.url,
        &form.system_text,
    ) {
        Ok(configuration) => configuration,
        Err(error) => {
            return configuration_form_error_response(StatusCode::BAD_REQUEST, "new", &error)
        }
    };
    let item = match configuration_item(&state, &configuration) {
        Ok(item) => item,
        Err(response) => return response,
    };
    let rendered = templates::render_created_configuration(&item)
        .and_then(|created| with_selector(&state, created));
    fragment_response(StatusCode::OK, rendered, "created configuration")
}

/// Handles GET requests for the list of built-in AI providers.
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

fn configuration_form_error_response(
    status: StatusCode,
    id: &str,
    error_message: &str,
) -> Response {
    fragment_response(
        status,
        render_configuration_form_error(id, error_message),
        error_message,
    )
}

/// Handles PUT requests for updating an AI configuration
async fn update_configuration_fragment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    let method = match supported_interaction_method_from_name(&form.interaction_method) {
        Some(method) => method,
        None => {
            return configuration_form_error_response(
                StatusCode::BAD_REQUEST,
                &id,
                "Invalid interaction method",
            );
        }
    };
    let configuration = match state.ai.configurations.update(
        &id,
        &form.name,
        method,
        &form.model_id,
        &form.url,
        &form.system_text,
    ) {
        Ok(configuration) => configuration,
        Err(error) => {
            return configuration_form_error_response(StatusCode::BAD_REQUEST, &id, &error);
        }
    };
    let item = match configuration_item(&state, &configuration) {
        Ok(item) => item,
        Err(response) => return response,
    };
    let rendered = templates::render_configuration_summary(&item)
        .and_then(|created| with_selector(&state, created));
    fragment_response(StatusCode::OK, rendered, "updated configuration")
}

/// Handles GET requests for the active AI configuration fragment.
async fn active_configuration_fragment(State(state): State<AppState>) -> Response {
    active_configuration_response(&state, StatusCode::OK, String::new())
}

/// Handles POST requests to select an AI configuration.
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

fn get_active_id(state: &AppState) -> Result<Option<String>, Response> {
    match state.ai.configurations.active() {
        Ok(active) => Ok(active.map(|c| c.id.clone())),
        Err(error) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            eprintln!("Could not get active configuration: {error}"),
        )
            .into_response()),
    }
}

fn active_configuration_response(state: &AppState, status: StatusCode, error: String) -> Response {
    let items = match configuration_list(&state.ai.configurations) {
        Ok(response) => list_items(&response),
        Err(response) => return response,
    };
    let active_id = match get_active_id(state) {
        Ok(id) => id,
        Err(response) => return response,
    };

    fragment_response(
        status,
        templates::render_active_configuration(&items, active_id.as_deref(), false, &error),
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
    Ok(ConfigurationListItem::from_configuration(
        configuration,
        has_api_key,
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
        Err(_) => return Ok(html), // TODO
    };
    let active_id = match state.ai.configurations.active() {
        Ok(active) => active.map(|c| c.id.clone()),
        Err(_) => return Ok(html), // TODO
    };
    templates::render_active_configuration(&items, active_id.as_deref(), false, "")
        .map(|selector| html + &selector)
}

fn summary_response(state: &AppState, item: &ConfigurationListItem) -> Response {
    let rendered = templates::render_configuration_summary(item)
        .and_then(|summary| with_selector(state, summary));
    fragment_response(StatusCode::OK, rendered, "AI configuration")
}

/// Handles GET requests for the closed AI configuration summary fragment.
async fn closed_configuration_summary_fragment(
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

/// Handles GET requests for the API key form fragment.
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

/// Handles POST requests to save an API key.
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

/// Handles POST requests to forget an API key.
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

/// Handles DELETE requests to remove an AI configuration.
async fn delete_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return configuration_form_error_response(status, &id, "Request origin not allowed.");
    }

    // The delete request comes only from the AI configuration edit form. On
    // success, we return a response that omits the now-gone summary line, and
    // includes an out-of-band fragment to update the active configuration list
    // control. On failure, we return an out-of-band form error and don't swap
    // the form out.

    if let Err(error) = state.ai.configurations.get(&id) {
        return configuration_form_error_response(StatusCode::NOT_FOUND, &id, &error);
    }
    let previous_key = match state.ai.configurations.has_api_key(&id) {
        Ok(true) => match state.ai.configurations.get_api_key(&id) {
            Ok(key) => Some(key),
            Err(error) => {
                return configuration_form_error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &id,
                    &error,
                )
            }
        },
        Ok(false) => None,
        Err(error) => {
            return configuration_form_error_response(StatusCode::SERVICE_UNAVAILABLE, &id, &error)
        }
    };
    if let Err(error) = state.ai.configurations.forget_api_key(&id) {
        return configuration_form_error_response(StatusCode::SERVICE_UNAVAILABLE, &id, &error);
    }
    if let Err(error) = state.ai.configurations.delete(&id) {
        let rollback = previous_key.map_or(Ok(()), |key| {
            state.ai.configurations.save_api_key(&id, &key)
        });
        if let Err(rollback_error) = rollback {
            return configuration_form_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &id,
                &format!("{error} API key rollback also failed: {rollback_error}"),
            );
        }
        return configuration_form_error_response(StatusCode::INTERNAL_SERVER_ERROR, &id, &error);
    }
    let rendered = with_selector(&state, String::new());
    fragment_response(StatusCode::OK, rendered, "deleted configuration")
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
            ConfigurationListItem::from_configuration(&summary.configuration, summary.api_key_saved)
        })
        .collect()
}

/// Handles GET requests for configuration page JavaScript.
async fn configuration_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/ai-configurations.js"),
    )
}

/// Handles GET requests for the main app JavaScript.
async fn app_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/app.js"),
    )
}

/// Handles GET requests for the htmx JavaScript library.
async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/htmx.min.js"),
    )
}

/// Handles GET requests for the htmx SSE JavaScript library.
async fn htmx_sse_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/hx-sse.min.js"),
    )
}

/// Handles GET requests for the main stylesheet.
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

/// Handles POST requests to configure automatic turns.
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

/// Handles GET requests for the server-sent events stream.
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

/// Handles POST requests for chat messages.
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

/// Handles POST requests to retry the last AI action.
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
        eprintln!("Host not allowed: {:?}", host);
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

/// If an Origin header is present, validates that it matches the expected local origin.
/// Returns an error status code if the origin is not allowed.
/// Should be used on POST, PUT, PATCH, and DELETE requests to ensure they originate from the local origin.
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
                interaction_method: genai::adapter::AdapterKind::OpenAIResp,
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
        let selector = templates::render_active_configuration(&items, Some("test-id-1"), false, "")
            .expect("selector should render");

        assert!(list.contains("Test configuration 2"));
        assert!(list.contains("OpenAI"));
        assert!(selector.contains("value=\"test-id-1\" checked"));
        assert!(!selector.contains("value=\"test-id-2\" checked"));
    }
}
