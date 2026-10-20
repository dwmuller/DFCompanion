use std::convert::Infallible;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Form, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        Html, IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::{
    ai::{Provider, UserSettings},
    utc_now, AppState, QueueItem, AI_RETRY_CATEGORY, USER_INPUT_CATEGORY,
};

const HTTP_ORIGIN: &str = "http://127.0.0.1:3000";
const HTTP_HOST: &str = "127.0.0.1:3000";

#[derive(Deserialize)]
struct ChatForm {
    message: String,
}

#[derive(Deserialize)]
struct SettingsForm {
    provider: Provider,
    model_id: String,
    base_url: Option<String>,
    api_key: Option<String>,
    forget_api_key: Option<bool>,
}

#[derive(Serialize)]
struct SettingsResponse {
    provider: Provider,
    model_id: String,
    base_url: String,
    api_key_saved: bool,
    privacy_policy_url: Option<&'static str>,
}

#[derive(Deserialize)]
struct PluginEvent {
    category: String,
    real_world_time_utc: SimulationTimestampUtc,
    game_world_time: SimulationTimestamp,
    description: String,
}

#[derive(Deserialize)]
struct SimulationTimestamp {
    year: i32,
    month: i32,
    day: i32,
    tick: i32,
}

#[derive(Deserialize)]
struct SimulationTimestampUtc {
    seconds: i64,
    nanos: i32,
}

#[derive(Serialize)]
struct PluginEventResult {
    status: &'static str,
    message: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/chat", post(chat))
        .route("/retry", post(retry))
        .route(
            "/api/settings",
            get(get_settings)
                .post(update_settings)
                .layer(DefaultBodyLimit::max(8_192)),
        )
        .route(
            "/api/plugin/events",
            post(plugin_event).layer(DefaultBodyLimit::max(16_384)),
        )
        .route("/events", get(events))
        .route("/assets/app.js", get(app_script))
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

    let Some(expected_token) = state.plugin_token.as_deref() else {
        return plugin_event_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Plugin event authentication is not configured.",
        );
    };

    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected_token);
    if !authorized {
        return plugin_event_response(
            StatusCode::UNAUTHORIZED,
            "rejected",
            "Missing or invalid plugin token.",
        );
    }

    let category = event.category.trim();
    if category.is_empty() || category.len() > 128 {
        return plugin_event_response(
            StatusCode::BAD_REQUEST,
            "rejected",
            "Category must contain between 1 and 128 bytes.",
        );
    }
    if matches!(category, USER_INPUT_CATEGORY | AI_RETRY_CATEGORY) {
        return plugin_event_response(
            StatusCode::BAD_REQUEST,
            "rejected",
            "Category is reserved for Companion chat controls.",
        );
    }
    let description = event.description.trim();
    if description.is_empty() {
        return plugin_event_response(
            StatusCode::BAD_REQUEST,
            "rejected",
            "Event description is required.",
        );
    }
    if description.len() > 8_000 {
        return plugin_event_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "rejected",
            "Event description exceeds the 8000-byte limit.",
        );
    }

    let real_world_time_utc = prost_types::Timestamp {
        seconds: event.real_world_time_utc.seconds,
        nanos: event.real_world_time_utc.nanos,
    };
    if !crate::validate_timestamp(&real_world_time_utc) {
        return plugin_event_response(
            StatusCode::BAD_REQUEST,
            "rejected",
            "UTC timestamp is out of range.",
        );
    }
    let game_world_time = crate::api::SimulationTimestamp {
        year: event.game_world_time.year,
        month: event.game_world_time.month,
        day: event.game_world_time.day,
        tick: event.game_world_time.tick,
    };
    if game_world_time.year < 0
        || game_world_time.month < 0
        || game_world_time.day < 0
        || game_world_time.tick < 0
    {
        return plugin_event_response(
            StatusCode::BAD_REQUEST,
            "rejected",
            "Game-world timestamp fields must be non-negative.",
        );
    }

    let item = QueueItem {
        category: category.to_owned(),
        real_world_time_utc,
        game_world_time: Some(game_world_time),
        text: description.to_owned(),
    };

    match state.queue_tx.try_send(item) {
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

async fn index(State(state): State<AppState>) -> impl IntoResponse {
    let command = state
        .plugin_token
        .as_deref()
        .map(|token| format!("dfcompanion {token}"))
        .unwrap_or_else(|| "DFHack command unavailable.".to_owned());
    let page = include_str!("../static/index.html").replace("{{DFHACK_COMMAND}}", &command);
    ([(header::CACHE_CONTROL, "no-store")], Html(page))
}

async fn get_settings(State(state): State<AppState>) -> Response {
    let settings = match state.ai.settings.get() {
        Ok(settings) => settings,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };
    let api_key_saved = match state.ai.has_api_key(settings.provider, &settings.base_url) {
        Ok(saved) => saved,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    (
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(to_settings_response(settings, api_key_saved)),
    )
        .into_response()
}

async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(form): axum::Json<SettingsForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }

    let model_id = form.model_id.trim();
    if model_id.is_empty() || model_id.len() > 256 {
        return (
            StatusCode::BAD_REQUEST,
            "Model ID must contain between 1 and 256 bytes.",
        )
            .into_response();
    }
    if let Some(key) = &form.api_key {
        if key.len() > 4_096 {
            return (StatusCode::PAYLOAD_TOO_LARGE, "API key is too long.").into_response();
        }
    }
    let base_url = match form.provider {
        Provider::Anthropic => form.provider.default_base_url().to_owned(),
        Provider::OpenAi | Provider::Groq => form.provider.default_base_url().to_owned(),
        Provider::OpenAiCompatible => form
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_owned(),
    };
    if form.provider == Provider::OpenAiCompatible {
        if let Err(error) = crate::ai::validate_compatible_base_url(&base_url) {
            return (StatusCode::BAD_REQUEST, error).into_response();
        }
    }

    let settings = match state
        .ai
        .settings
        .update(form.provider, model_id.to_owned(), base_url)
    {
        Ok(settings) => settings,
        Err(error) => return (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    };

    if form.forget_api_key.unwrap_or(false) {
        if let Err(error) = state
            .ai
            .forget_api_key(settings.provider, &settings.base_url)
        {
            return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
        }
    } else if let Some(key) = form.api_key.filter(|key| !key.trim().is_empty()) {
        if let Err(error) = state
            .ai
            .save_api_key(settings.provider, &settings.base_url, key.trim())
        {
            return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
        }
    }

    let api_key_saved = match state.ai.has_api_key(settings.provider, &settings.base_url) {
        Ok(saved) => saved,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    (
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(to_settings_response(settings, api_key_saved)),
    )
        .into_response()
}

fn to_settings_response(settings: UserSettings, api_key_saved: bool) -> SettingsResponse {
    SettingsResponse {
        provider: settings.provider,
        model_id: settings.model_id,
        base_url: settings.base_url,
        api_key_saved,
        privacy_policy_url: settings.provider.privacy_policy_url(),
    }
}

async fn app_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/app.js"),
    )
}

async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.notifications_tx.subscribe()).map(|notification| {
        Ok(match notification {
            Ok(message) => Event::default().data(message),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                Event::default().event("warning").data(format!(
                    "{count} UI notifications were missed while the browser was disconnected."
                ))
            }
        })
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
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

    let timestamp = match utc_now() {
        Ok(timestamp) => timestamp,
        Err(error) => {
            eprintln!("could not construct UTC timestamp for chat input: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not timestamp the message.",
            )
                .into_response();
        }
    };
    let item = QueueItem {
        category: USER_INPUT_CATEGORY.to_owned(),
        real_world_time_utc: timestamp,
        game_world_time: None,
        text: message.to_owned(),
    };

    match state.queue_tx.try_send(item) {
        Ok(()) => {
            state.notify(format!("You: {message}"));
            (StatusCode::OK, "<span>Message queued.</span>").into_response()
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
    let item = QueueItem {
        category: AI_RETRY_CATEGORY.to_owned(),
        real_world_time_utc: match utc_now() {
            Ok(timestamp) => timestamp,
            Err(error) => {
                eprintln!("could not timestamp AI retry request: {error}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not timestamp the retry request.",
                )
                    .into_response();
            }
        },
        game_world_time: None,
        text: String::new(),
    };
    match state.queue_tx.try_send(item) {
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
