use std::convert::Infallible;

use axum::{
    body::Body,
    extract::{Form, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        Html, IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::{utc_now, AppState, QueueItem, USER_INPUT_CATEGORY};

const HTTP_ORIGIN: &str = "http://127.0.0.1:3000";
const HTTP_HOST: &str = "127.0.0.1:3000";

#[derive(Deserialize)]
struct ChatForm {
    message: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/chat", post(chat))
        .route("/events", get(events))
        .route("/assets/htmx.min.js", get(htmx_script))
        .route("/assets/app.js", get(app_script))
        .route("/assets/style.css", get(stylesheet))
        .layer(middleware::from_fn(validate_local_request))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn htmx_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/htmx.min.js"),
    )
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
