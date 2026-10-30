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
use genai::adapter::AdapterKind;
use serde::{Deserialize, Serialize};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::{
    ai_config::{AiConfiguration, ConfigurationRepository},
    ai_providers::built_in_providers,
    web_templates::{self as templates, ConfigurationListItem, CreateFormValues, RenderError},
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
            "/fragments/ai-configurations/:id/key",
            post(forget_api_key_fragment).layer(DefaultBodyLimit::max(8_192)),
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
        .route(
            "/api/timed-turns",
            get(get_timed_turns).put(set_timed_turns),
        )
        .route("/assets/app.js", get(app_script))
        .route("/assets/marked.min.js", get(marked_script))
        .route("/assets/purify.min.js", get(purify_script))
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

    // The selector shows key status, which this response may have changed.
    let rendered = templates::render_configuration_list(&items).and_then(|list| {
        templates::render_active_configuration(&items, true, "").map(|selector| list + &selector)
    });
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
    if form
        .api_key
        .as_ref()
        .is_some_and(|key| key.len() > crate::ai_config::MAX_API_KEY_BYTES)
    {
        return create_form_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            CreateFormValues {
                name: form.name,
                url: form.url,
                model_id: form.model_id,
                system_text: form.system_text,
            },
            "API key is too long.",
        );
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
    if let Some(key) = form.api_key.filter(|key| !key.trim().is_empty()) {
        if let Err(error) = state
            .ai
            .configurations
            .save_api_key(&configuration.id, &key)
        {
            let cleanup = state.ai.configurations.delete(&configuration.id);
            let detail = cleanup
                .err()
                .map(|cleanup_error| format!(" Configuration cleanup also failed: {cleanup_error}"))
                .unwrap_or_default();
            return (StatusCode::SERVICE_UNAVAILABLE, format!("{error}{detail}")).into_response();
        }
    }
    let has_api_key = match state.ai.configurations.has_api_key(&configuration.id) {
        Ok(has_api_key) => has_api_key,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    let is_active = match state.ai.configurations.active() {
        Ok(Some(config)) => config.id == configuration.id,
        Ok(None) => false,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    let item = ConfigurationListItem::from_configuration(&configuration, has_api_key, is_active);
    let rendered = templates::render_created_configuration(&item).and_then(|created| {
        let items = match configuration_list(&state.ai.configurations) {
            Ok(response) => list_items(&response),
            Err(_) => return Ok(created),
        };
        templates::render_active_configuration(&items, true, "").map(|selector| created + &selector)
    });
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

async fn forget_api_key_fragment(
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

    if let Err(error) = state.ai.configurations.forget_api_key(&id) {
        return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
    }

    configuration_list_fragment(State(state)).await
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

async fn marked_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/marked.min.js"),
    )
}

async fn purify_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/purify.min.js"),
    )
}

async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/vendor/htmx.min.js"),
    )
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

/// Game-event echoes get their own SSE event type so the page can hide them.
fn notification_event(message: String) -> Event {
    match message.strip_prefix(GAME_EVENT_PREFIX) {
        Some(text) => return Event::default().event("game_event").data(text),
        None => {}
    }
    match message.strip_prefix(crate::UNSOLICITED_PREFIX) {
        Some(text) => Event::default().event("unsolicited").data(text),
        None => Event::default().data(message),
    }
}

#[derive(Deserialize)]
struct TimedTurnsForm {
    enabled: bool,
}

async fn get_timed_turns(State(state): State<AppState>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({"enabled": state.timed_turns_enabled()}))
}

async fn set_timed_turns(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(form): axum::Json<TimedTurnsForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    state
        .timed_turns
        .store(form.enabled, std::sync::atomic::Ordering::Relaxed);
    get_timed_turns(State(state)).await.into_response()
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let (backlog, live) = state.history.subscribe();
    let backlog = tokio_stream::iter(
        backlog
            .into_iter()
            .map(|message| Ok(notification_event(message))),
    );
    let live = BroadcastStream::new(live).map(|notification| {
        Ok(match notification {
            Ok(message) => notification_event(message),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                Event::default().event("warning").data(format!(
                    "{count} UI notifications were missed while the browser was disconnected."
                ))
            }
        })
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
            (StatusCode::OK, "Message queued.").into_response()
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
        assert!(list.contains("API key saved"));
        assert!(selector.contains("value=\"test-id-1\" selected"));
        assert!(!selector.contains("value=\"test-id-2\" selected"));
    }
}
