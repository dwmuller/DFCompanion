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
    notifications::{DisplayMessage, DisplayMessageKind, HistoryEvent, InputState, NotificationId},
    web::templates::{render_configuration_form_error, ConfigurationListItem, RenderError},
};

const HTTP_ORIGIN: &str = "http://127.0.0.1:3000";
const HTTP_HOST: &str = "127.0.0.1:3000";

#[derive(Deserialize)]
struct ChatForm {
    message: String,
}

#[derive(Deserialize)]
struct RollbackForm {
    /// The current contents of the chat editor, from `hx-include`.
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

/// A game loading-state push from the DFHack plugin.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginLifecycle {
    world_loaded: bool,
    map_loaded: bool,
    fortress_name: String,
}

/// Logs every HTTP request and response to stdout. Enabled by `--trace-http`.
pub fn with_request_tracing(router: Router) -> Router {
    use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
    use tracing::Level;

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
        .route("/rollback/:id", post(rollback))
        .route("/rollback-floor-events", get(rollback_floor_events))
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
        .route(
            "/api/plugin/lifecycle",
            post(plugin_lifecycle).layer(DefaultBodyLimit::max(4_096)),
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

    match app.event_queue().try_send(QueueItem::GameEvent(event)) {
        Ok(()) => plugin_event_response(StatusCode::OK, "accepted", "Event accepted."),
        Err(message) => {
            app.notifier().notify(DisplayMessage::new(
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

/// Handles game loading-state pushes from the DFHack plugin.
async fn plugin_lifecycle(
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<PluginLifecycle>,
) -> impl IntoResponse {
    if headers.contains_key(header::ORIGIN) {
        return plugin_event_response(
            StatusCode::FORBIDDEN,
            "rejected",
            "Browser-originated requests are not accepted.",
        );
    }
    app.update_lifecycle(
        payload.world_loaded,
        payload.map_loaded,
        Some(payload.fortress_name),
    );
    plugin_event_response(StatusCode::OK, "accepted", "Game state accepted.")
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
        let _ = app.event_queue().try_send(QueueItem::Orient);
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
#[allow(clippy::result_large_err)]
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

#[allow(clippy::result_large_err)]
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
    let (items, active_id) = configuration_items(&app.ai.configurations)?;
    templates::render_active_configuration(&items, active_id.as_deref(), true, "")
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
/// `live` is false while replaying history, so replays never beep. The SSE
/// event id is `epoch:id`, so a reconnecting browser's `Last-Event-ID` always
/// names both the entry and the log generation it came from.
/// Whether a notification should offer a rollback button: only user inputs at
/// or after the rollback floor are still eligible.
fn rollback_eligible(message: &DisplayMessage, id: u64, floor: Option<NotificationId>) -> bool {
    message.kind == DisplayMessageKind::User
        && message.input_state.is_some()
        && floor.is_some_and(|floor| id >= floor.0)
}

fn notification_events(
    epoch: u64,
    id: u64,
    message: &DisplayMessage,
    live: bool,
    rollback_eligible: bool,
) -> Vec<Event> {
    let html = match templates::render_notification(id, message, rollback_eligible) {
        Ok(html) => html,
        Err(error) => {
            eprintln!("Could not render notification: {error}");
            return Vec::new();
        }
    };
    // Unnamed events are swapped into the page; named ones become DOM events.
    let mut events = vec![Event::default().id(format!("{epoch}:{id}")).data(html)];
    if message.beep && live {
        events.push(Event::default().event("beep").data(""));
    }
    events
}

/// The out-of-band swap that updates an already-rendered notification's
/// state badge in place.
fn state_change_events(id: u64, state: InputState) -> Vec<Event> {
    match templates::render_notification_state(id, state) {
        Ok(html) => vec![Event::default().data(html)],
        Err(error) => {
            eprintln!("Could not render notification state: {error}");
            Vec::new()
        }
    }
}

/// Parses a `Last-Event-ID` header value of the form `epoch:id`.
fn parse_cursor(value: &str) -> Option<(u64, u64)> {
    let (epoch, id) = value.split_once(':')?;
    Some((epoch.parse().ok()?, id.parse().ok()?))
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
    // A reconnecting browser says where it left off, as `epoch:id`, so it
    // isn't sent messages it already has.
    let cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_cursor);
    let subscription = app.history.subscribe_from(cursor);
    let epoch = subscription.epoch;

    let floor = app.rollback_floor();
    let mut backlog_events: Vec<Result<Event, Infallible>> = Vec::new();
    if subscription.reset {
        // The browser's remembered ids no longer exist: tell it to drop
        // everything before replaying the current, complete backlog.
        backlog_events.push(Ok(Event::default().event("reset").data("")));
    }
    backlog_events.extend(
        subscription
            .backlog
            .into_iter()
            .flat_map(|(id, message)| {
                let eligible = rollback_eligible(&message, id, floor);
                notification_events(epoch, id, &message, false, eligible)
            })
            .map(Ok),
    );
    let backlog = tokio_stream::iter(backlog_events);

    // Tracks the epoch across the live stream, since a `Reset` partway
    // through changes which epoch subsequent `Added` ids belong to.
    let epoch_cell = std::cell::Cell::new(epoch);
    let live_app = app.clone();
    let live = BroadcastStream::new(subscription.receiver).flat_map(move |notification| {
        let events = match notification {
            Ok(HistoryEvent::Added(id, message)) => {
                let eligible = rollback_eligible(&message, id, live_app.rollback_floor());
                notification_events(epoch_cell.get(), id, &message, true, eligible)
            }
            Ok(HistoryEvent::StateChanged(id, state)) => state_change_events(id, state),
            Ok(HistoryEvent::Reset(new_epoch)) => {
                epoch_cell.set(new_epoch);
                // A truncation removed the tail of the log, not all of it. Tell
                // the browser to drop what it has, then resend the surviving
                // backlog so it can rebuild.
                let floor = live_app.rollback_floor();
                let mut events = vec![Event::default().event("reset").data("")];
                let (backlog, _) = live_app.history.subscribe(None);
                events.extend(backlog.into_iter().flat_map(|(id, message)| {
                    let eligible = rollback_eligible(&message, id, floor);
                    notification_events(new_epoch, id, &message, false, eligible)
                }));
                events
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                vec![Event::default().data(
                    templates::render_notification(
                        0,
                        &DisplayMessage::new(
                            DisplayMessageKind::Warning,
                            format!(
                                "{count} UI notifications were missed while the browser was disconnected."
                            ),
                        ),
                        false,
                    )
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

    match app.is_chat_message(message) {
        Ok(_) => {}
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };

    match app
        .event_queue()
        .try_send(QueueItem::UserInput(message.to_owned()))
    {
        Ok(()) => fragment_response(StatusCode::OK, templates::render_chat_sent(), "chat status"),
        Err(error) => {
            app.notifier().notify(DisplayMessage::new(
                DisplayMessageKind::Warning,
                error.clone(),
            ));
            (StatusCode::SERVICE_UNAVAILABLE, error).into_response()
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
            let chat_state = if initial || receiver.changed().await.is_ok() {
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

/// Handles POST requests to roll the conversation back to before one input.
/// The current editor text is included so dropped user requests can be put
/// ahead of it.
async fn rollback(
    State(app): State<Arc<CompanionApp>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<RollbackForm>,
) -> Response {
    if let Err(status) = validate_local_origin(&headers) {
        return (status, "Request origin not allowed.").into_response();
    }
    match app.rollback(NotificationId(id), form.message) {
        Ok(combined) => fragment_response(
            StatusCode::OK,
            templates::render_rollback_reply(&combined),
            "rollback reply",
        ),
        Err(error) => (StatusCode::CONFLICT, error).into_response(),
    }
}

/// Streams the rollback floor as it changes, so the page can drop Rollback
/// buttons that are no longer eligible. Named `rollback-floor` events carry
/// the floor's id, or nothing when there is no floor.
async fn rollback_floor_events(
    State(app): State<Arc<CompanionApp>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold(
        (app.subscribe_rollback_floor(), true),
        |(mut receiver, initial)| async move {
            if !initial && receiver.changed().await.is_err() {
                return None;
            }
            let floor = *receiver.borrow_and_update();
            let data = floor.map(|id| id.0.to_string()).unwrap_or_default();
            Some((
                Ok(Event::default().event("rollback-floor").data(data)),
                (receiver, false),
            ))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
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
mod tests;
