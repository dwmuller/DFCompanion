mod templates;

use std::{convert::Infallible, sync::Arc};

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
    app::CompanionApp,
    events::{GameEvent, QueueItem},
    notifications::{DisplayMessage, DisplayMessageKind},
    web::templates::{render_configuration_form_error, ConfigurationListItem, RenderError},
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

pub fn router(app: Arc<CompanionApp>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ai-configurations", get(configurations_page))
        .route("/chat", post(chat))
        .route("/chat/resume", post(resume_chat))
        .route("/chat-state", get(chat_state_events))
        .route("/conversation.md", get(copy_conversation))
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
        .with_state(app)
}

/// Handles POST requests for events from DFHack plugin.
async fn plugin_event(
    State(app): State<Arc<CompanionApp>>,
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

    match app.queue_tx.try_send(QueueItem::GameEvent(event)) {
        Ok(()) => plugin_event_response(StatusCode::OK, "accepted", "Event accepted."),
        Err(message) => {
            app.get_notifier().notify(DisplayMessage::new(
                DisplayMessageKind::Warning,
                message.clone(),
            ));
            plugin_event_response(StatusCode::TOO_MANY_REQUESTS, "queue_full", &message)
        }
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
fn render_error_response(what: &str, error: RenderError) -> Response {
    eprintln!("Could not render {what}: {error}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Could not render {what}."),
    )
        .into_response()
}

fn configuration_items(
    repository: &ConfigurationRepository,
) -> Result<(Vec<ConfigurationListItem>, Option<String>), RenderError> {
    let (configurations, active_id) = repository
        .list()
        .map_err(|error| RenderError::new(&error.to_string()))?;

    let items = configurations
        .into_iter()
        .map(|configuration| {
            let api_key_saved = repository
                .has_api_key(&configuration.id)
                .map_err(|error| RenderError::new(&error.to_string()))?;

            Ok(ConfigurationListItem::from_configuration(
                &configuration,
                api_key_saved,
            ))
        })
        .collect::<Result<Vec<_>, RenderError>>()?;
    Ok((items, active_id))
}

/// Handles GET requests for the index page.
async fn index(State(app): State<Arc<CompanionApp>>) -> Response {
    match configuration_items(&app.ai.configurations) {
        Ok((items, active_id)) => fragment_response(
            StatusCode::OK,
            templates::render_index(&items, active_id.as_deref(), app.chat_state()),
            "page",
        ),
        Err(err) => render_error_response("index", err),
    }
}

/// Handles GET requests for the AI configurations page.
async fn configurations_page(State(app): State<Arc<CompanionApp>>) -> Response {
    match configuration_items(&app.ai.configurations) {
        Ok((items, active_id)) => fragment_response(
            StatusCode::OK,
            templates::render_configurations_page(&items, active_id.as_deref()),
            "page",
        ),
        Err(err) => render_error_response("configurations page", err),
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

/// Handles GET requests for the AI configuration list fragment
async fn configuration_list_fragment(State(app): State<Arc<CompanionApp>>) -> Response {
    let (items, _active_id) = match configuration_items(&app.ai.configurations) {
        Ok(items) => items,
        Err(err) => return render_error_response("AI configurations", err),
    };

    let rendered =
        templates::render_configuration_list(&items).and_then(|list| with_selector(&app, list));
    fragment_response(StatusCode::OK, rendered, "AI configurations")
}

/// Handles GET requests for the new AI configuration form fragment
async fn new_configuration_form_fragment(State(_app): State<Arc<CompanionApp>>) -> Response {
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
    State(app): State<Arc<CompanionApp>>,
    Path(id): Path<String>,
) -> Response {
    let config = match app.ai.configurations.try_get(&id) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "AI configuration was not found.").into_response()
        }
        Err(response) => return (StatusCode::INTERNAL_SERVER_ERROR, response).into_response(),
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
async fn closed_new_configuration_form_fragment(State(_app): State<Arc<CompanionApp>>) -> Response {
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
    State(app): State<Arc<CompanionApp>>,
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
    let configuration = match app.ai.configurations.create(
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
    let item = match configuration_item(&app, &configuration) {
        Ok(item) => item,
        Err(response) => return response,
    };
    let rendered = templates::render_created_configuration(&item)
        .and_then(|created| with_selector(&app, created));
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
    State(app): State<Arc<CompanionApp>>,
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
    let configuration = match app.ai.configurations.update(
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
    let item = match configuration_item(&app, &configuration) {
        Ok(item) => item,
        Err(response) => return response,
    };
    let rendered = templates::render_configuration_summary(&item)
        .and_then(|created| with_selector(&app, created));
    fragment_response(StatusCode::OK, rendered, "updated configuration")
}

/// Handles GET requests for the active AI configuration fragment.
async fn active_configuration_fragment(State(app): State<Arc<CompanionApp>>) -> Response {
    active_configuration_response(&app, StatusCode::OK, String::new())
}

/// Handles POST requests to select an AI configuration.
async fn select_configuration_fragment(
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    Form(form): Form<SelectConfigurationForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }

    let id = Some(form.id.as_str()).filter(|id| !id.is_empty());
    if let Err(error) = app.ai.configurations.set_active(id) {
        return active_configuration_response(&app, StatusCode::BAD_REQUEST, error);
    }
    if id.is_some() {
        let _ = app.queue_tx.try_send(QueueItem::Orient);
    }

    active_configuration_response(&app, StatusCode::OK, String::new())
}

fn active_configuration_response(
    app: &CompanionApp,
    status: StatusCode,
    error: String,
) -> Response {
    let (items, active_id) = match configuration_items(&app.ai.configurations) {
        Ok(items) => items,
        Err(error) => return render_error_response("configuration items", error),
    };
    fragment_response(
        status,
        templates::render_active_configuration(&items, active_id.as_deref(), false, &error),
        "active AI configuration",
    )
}

/// Looks up a configuration and builds its list item, or the error response.
fn configuration_item(
    app: &CompanionApp,
    configuration: &AiConfiguration,
) -> Result<ConfigurationListItem, Response> {
    let has_api_key = app
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
    app: &CompanionApp,
    id: &str,
) -> Result<ConfigurationListItem, Response> {
    let configuration = app
        .ai
        .configurations
        .try_get(id)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error).into_response())?;
    let configuration = match configuration {
        Some(config) => config,
        None => {
            return Err((StatusCode::NOT_FOUND, "AI configuration was not found.").into_response())
        }
    };
    configuration_item(app, &configuration)
}

fn as_partial(html: &str, target: &str, swap: &str) -> String {
    format!("\n<hx-partial hx-target=\"{target}\" hx-swap=\"{swap}\">{html}</hx-partial>\n")
}

fn get_active_configuration_selector(app: &CompanionApp) -> Result<String, RenderError> {
    let (items, active_id) = match configuration_items(&app.ai.configurations) {
        Ok(items) => items,
        Err(re) => return Err(re),
    };
    match templates::render_active_configuration(&items, active_id.as_deref(), true, "") {
        Ok(selector) => Ok(selector),
        Err(error) => Err(error),
    }
}

/// Adds the active-configuration selector to the given HTML as an out-of-band
/// swap. Use this when something about the selection list in the may have
/// changed.
///
/// Argument `html` is the replacement for the triggering element. Even if
/// empty, it's assumed that the intent is to replace it. If you want to send
/// the selector only, leaving the triggering element unchanged, use the result
/// of get_active_configuration_selector directly. Note that this behavior can
/// be affected by htmx global settings and the hx-swap swapEmpty modifier.
fn with_selector(app: &CompanionApp, html: String) -> Result<String, RenderError> {
    // Note that there was a bug in htmx 4 where deleting the triggering element
    // would cause following partials to not find their targets. So we prepend,
    // rather than append, the selector to the HTML.
    //
    // Note that hx-target="this" does not seem to work as expected.
    let selector = get_active_configuration_selector(app)?;
    if html.is_empty() {
        Ok(selector + &as_partial("", "closest li", "delete"))
    } else {
        Ok(selector + &html)
    }
}

fn summary_response(app: &CompanionApp, item: &ConfigurationListItem) -> Response {
    let rendered = templates::render_configuration_summary(item)
        .and_then(|summary| with_selector(app, summary));
    fragment_response(StatusCode::OK, rendered, "AI configuration")
}

/// Handles GET requests for the closed AI configuration summary fragment.
async fn closed_configuration_summary_fragment(
    State(app): State<Arc<CompanionApp>>,
    Path(id): Path<String>,
) -> Response {
    match existing_configuration_item(&app, &id) {
        Ok(item) => fragment_response(
            StatusCode::OK,
            templates::render_configuration_summary(&item),
            "AI configuration",
        ),
        Err(response) => response,
    }
}

/// Handles GET requests for the API key form fragment.
async fn api_key_form_fragment(
    State(app): State<Arc<CompanionApp>>,
    Path(id): Path<String>,
) -> Response {
    match existing_configuration_item(&app, &id) {
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
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<ApiKeyForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    let item = match existing_configuration_item(&app, &id) {
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

    if let Err(error) = app.ai.configurations.save_api_key(&id, &form.api_key) {
        return fragment_response(
            StatusCode::SERVICE_UNAVAILABLE,
            templates::render_api_key_form(&item, &error),
            "API key form",
        );
    }
    match existing_configuration_item(&app, &id) {
        Ok(item) => summary_response(&app, &item),
        Err(response) => response,
    }
}

/// Handles POST requests to forget an API key.
async fn forget_api_key_fragment(
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    if let Err(response) = existing_configuration_item(&app, &id) {
        return response;
    }
    if let Err(error) = app.ai.configurations.forget_api_key(&id) {
        return (StatusCode::SERVICE_UNAVAILABLE, error).into_response();
    }
    match existing_configuration_item(&app, &id) {
        Ok(item) => summary_response(&app, &item),
        Err(response) => response,
    }
}

/// Handles DELETE requests to remove an AI configuration.
async fn delete_configuration(
    State(app): State<Arc<CompanionApp>>,
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

    match app.ai.configurations.try_get(&id) {
        Err(error) => return configuration_form_error_response(StatusCode::NOT_FOUND, &id, &error),
        Ok(None) => (), // Already gone. Odd, but no harm done. Desired state already achieved.
        Ok(Some(_)) => {
            // If the configuration has an associated API key, we attempt to
            // retrieve it so that we can roll it back in case the deletion
            // fails.
            let previous_key = match app.ai.configurations.has_api_key(&id) {
                Ok(true) => match app.ai.configurations.get_api_key(&id) {
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
                    return configuration_form_error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        &id,
                        &error,
                    )
                }
            };
            if let Err(error) = app.ai.configurations.forget_api_key(&id) {
                return configuration_form_error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &id,
                    &error,
                );
            }
            if let Err(error) = app.ai.configurations.delete(&id) {
                let rollback = previous_key
                    .map_or(Ok(()), |key| app.ai.configurations.save_api_key(&id, &key));
                if let Err(rollback_error) = rollback {
                    return configuration_form_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &id,
                        &format!("{error} API key rollback also failed: {rollback_error}"),
                    );
                }
                return configuration_form_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &id,
                    &error,
                );
            }
        }
    }
    // Note that since we're sending an out-of-band response, and we want the
    // current triggering element to go away, we have to send something trivial
    // to replace it.
    let rendered = with_selector(&app, "".to_string());
    fragment_response(StatusCode::OK, rendered, "deleted configuration")
}

/// Handles GET requests for configuration page JavaScript.
async fn configuration_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/static/ai-configurations.js"),
    )
}

/// Handles GET requests for the main app JavaScript.
async fn app_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/static/app.js"),
    )
}

/// Handles GET requests for the htmx JavaScript library.
async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/static/vendor/htmx.min.js"),
    )
}

/// Handles GET requests for the htmx SSE JavaScript library.
async fn htmx_sse_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/static/vendor/hx-sse.min.js"),
    )
}

/// Handles GET requests for the main stylesheet.
async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("web/static/style.css"),
    )
}

/// Turns a recorded notification into the HTML list item the page appends.
/// `live` is false while replaying history, so replays never beep.
fn notification_events(id: u64, message: &DisplayMessage, live: bool) -> Vec<Event> {
    let html = match templates::render_notification(message) {
        Ok(html) => html,
        Err(error) => {
            eprintln!("Could not render notification: {error}");
            return Vec::new();
        }
    };
    // Unnamed events are swapped into the page; named ones become DOM events.
    let mut events = vec![Event::default().id(id.to_string()).data(html)];
    if message.beep && live {
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
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    Form(form): Form<AutomaticTurnsForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    app.event_turns.store(
        form.event_turns.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    app.periodic_turns.store(
        form.periodic_turns.is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Handles GET requests for the server-sent events stream.
async fn events(
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    // A reconnecting browser says where it left off, so it isn't sent
    // messages it already has.
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let (backlog, live) = app.history.subscribe(last_event_id);
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
                    templates::render_notification(&DisplayMessage::new(
                        DisplayMessageKind::Warning,
                        format!(
                            "{count} UI notifications were missed while the browser was disconnected."
                        ),
                    ))
                    .unwrap_or_default(),
                )]
            }
        };
        tokio_stream::iter(events.into_iter().map(Ok))
    });

    Sse::new(backlog.chain(live)).keep_alive(KeepAlive::default())
}

/// Serves the user/companion transcript from typed message history, not rendered DOM.
async fn copy_conversation(State(app): State<Arc<CompanionApp>>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        app.history.display_history_markdown(),
    )
        .into_response()
}

/// Handles POST requests for chat messages.
async fn chat(
    State(app): State<Arc<CompanionApp>>,
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

    match app.chat(message.to_owned()) {
        Ok(()) => {
            let kind = if crate::chat_commands::ChatCommand::parse(message).is_some() {
                DisplayMessageKind::Diagnostic
            } else {
                DisplayMessageKind::User
            };
            app.get_notifier()
                .notify(DisplayMessage::new(kind, message));
            fragment_response(StatusCode::OK, templates::render_chat_sent(), "chat status")
        }
        Err(err) => {
            app.get_notifier().notify(DisplayMessage::new(
                DisplayMessageKind::Warning,
                err.to_string(),
            ));
            (StatusCode::SERVICE_UNAVAILABLE, err).into_response()
        }
    }
}

/// Clears the blocked chat state; the state SSE stream updates the page.
async fn resume_chat(State(app): State<Arc<CompanionApp>>, headers: HeaderMap) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    app.resume_if_blocked();
    StatusCode::NO_CONTENT.into_response()
}

/// Streams the current chat state and subsequent changes as server-rendered HTML.
async fn chat_state_events(
    State(app): State<Arc<CompanionApp>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold(
        (app.subscribe_chat_state(), true),
        |(mut receiver, initial)| async move {
            let chat_state = if initial {
                Some(*receiver.borrow_and_update())
            } else if receiver.changed().await.is_ok() {
                Some(*receiver.borrow_and_update())
            } else {
                None
            }?;
            let event = match templates::render_chat_state_content(chat_state) {
                Ok(html) => Event::default().data(html),
                Err(error) => {
                    eprintln!("Could not render chat state: {error}");
                    Event::default().data("Chat status is unavailable. Refresh the page.")
                }
            };
            Some((Ok(event), (receiver, false)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Handles POST requests to retry the last AI action.
async fn retry(State(app): State<Arc<CompanionApp>>, headers: HeaderMap) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    match app.queue_tx.try_send(QueueItem::AiRetry) {
        Ok(()) => (StatusCode::OK, "Retry queued.").into_response(),
        Err(message) => (StatusCode::SERVICE_UNAVAILABLE, message).into_response(),
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
    use crate::app::{ChatState, CompanionApp};
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[test]
    fn list_items_maps_summaries_and_marks_the_active_configuration() {
        let items = vec![
            AiConfiguration {
                id: "test-id-1".to_owned(),
                name: "Test configuration 1".to_owned(),
                interaction_method: genai::adapter::AdapterKind::OpenAIResp,
                model_id: String::new(),
                url: String::new(),
                system_text: String::new(),
            },
            AiConfiguration {
                id: "test-id-2".to_owned(),
                name: "Test configuration 2".to_owned(),
                interaction_method: genai::adapter::AdapterKind::OpenAIResp,
                model_id: String::new(),
                url: String::new(),
                system_text: String::new(),
            },
        ]
        .into_iter()
        .map(|c| ConfigurationListItem::from_configuration(&c, false))
        .collect::<Vec<_>>();

        let list = templates::render_configuration_list(&items).expect("list should render");
        let selector = templates::render_active_configuration(&items, Some("test-id-1"), false, "")
            .expect("selector should render");

        assert!(list.contains("Test configuration 2"));
        assert!(list.contains("OpenAI"));
        assert!(selector.contains("value=\"test-id-1\" checked"));
        assert!(!selector.contains("value=\"test-id-2\" checked"));
    }

    #[tokio::test]
    async fn conversation_markdown_endpoint_uses_all_messages() {
        let (app, _, _) = CompanionApp::new();
        for message in [
            DisplayMessage::new(DisplayMessageKind::User, "Question"),
            DisplayMessage::new(DisplayMessageKind::GameEvent, "Arrival"),
            DisplayMessage::new(DisplayMessageKind::Diagnostic, "/tools"),
            DisplayMessage::new(DisplayMessageKind::AiUsage, "2 requests"),
            DisplayMessage::new(DisplayMessageKind::Status, "Connected"),
            DisplayMessage::new(DisplayMessageKind::Warning, "Heads up"),
            DisplayMessage::new(DisplayMessageKind::Companion, "**Answer**"),
        ] {
            app.get_notifier().notify(message);
        }
        for _ in 0..50 {
            if app.history.subscribe(None).0.len() == 7 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let response = router(app)
            .oneshot(
                Request::builder()
                    .uri("/conversation.md")
                    .header(header::HOST, HTTP_HOST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            "**You:**\n\nQuestion\n\n**Game event:**\n\nArrival\n\n**Diagnostic:**\n\n/tools\n\n**AI usage:**\n\n2 requests\n\n**Status:**\n\nConnected\n\n**Warning:**\n\nHeads up\n\n**Companion:**\n\n**Answer**"
        );
    }

    #[tokio::test]
    async fn slash_chat_input_is_a_diagnostic_not_a_user_turn() {
        let (app, mut queue_rx, notifications_tx) = CompanionApp::new();
        let mut notifications = notifications_tx.subscribe();
        let response = router(app)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header(header::HOST, HTTP_HOST)
                    .header(header::ORIGIN, HTTP_ORIGIN)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("message=%2Ftools"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            queue_rx.recv().await.unwrap(),
            QueueItem::UserInput("/tools".to_owned())
        );
        assert_eq!(
            notifications.recv().await.unwrap(),
            DisplayMessage::new(DisplayMessageKind::Diagnostic, "/tools")
        );
    }

    #[tokio::test]
    async fn resume_chat_requires_local_origin_and_clears_blocked_state() {
        let (app, queue_rx, _) = CompanionApp::new();
        let svc = router(app.clone());
        let status_response = svc
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/chat-state")
                    .header(header::HOST, HTTP_HOST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_response.status(), StatusCode::OK);
        assert!(status_response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        let mut status_events = status_response.into_body().into_data_stream();
        let initial_status =
            tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
                .await
                .expect("initial chat status should be sent")
                .unwrap()
                .unwrap();
        assert!(!String::from_utf8_lossy(&initial_status).contains("AI processing is blocked"));

        let mut state_changes = app.subscribe_chat_state();
        let worker = tokio::spawn(app.clone().run_companion_worker(queue_rx));
        app.chat("A request without an active configuration".to_owned())
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if *state_changes.borrow_and_update() == ChatState::Blocked {
                    break;
                }
                state_changes.changed().await.unwrap();
            }
        })
        .await
        .expect("chat should become blocked without an active configuration");

        let blocked_status =
            tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
                .await
                .expect("updated chat status should be sent")
                .unwrap()
                .unwrap();
        assert!(String::from_utf8_lossy(&blocked_status).contains("AI processing is blocked"));

        let rejected = svc
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/resume")
                    .header(header::HOST, HTTP_HOST)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
        assert_eq!(app.chat_state(), ChatState::Blocked);

        let resumed = svc
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/resume")
                    .header(header::HOST, HTTP_HOST)
                    .header(header::ORIGIN, HTTP_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::NO_CONTENT);
        assert_eq!(app.chat_state(), ChatState::Idle);
        let idle_status =
            tokio::time::timeout(std::time::Duration::from_secs(2), status_events.next())
                .await
                .expect("resumed chat status should be sent")
                .unwrap()
                .unwrap();
        assert!(!String::from_utf8_lossy(&idle_status).contains("AI processing is blocked"));
        worker.abort();
    }

    #[test]
    fn replay_and_live_messages_render_identically_and_only_live_beeps() {
        let message =
            DisplayMessage::new(DisplayMessageKind::Companion, "**Hello**").with_beep(true);
        let replay = notification_events(7, &message, false);
        let live = notification_events(7, &message, true);

        assert_eq!(replay.len(), 1);
        assert_eq!(live.len(), 2);
        assert_eq!(format!("{:?}", replay[0]), format!("{:?}", live[0]));
    }
}
