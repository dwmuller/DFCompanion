use std::time::{SystemTime, UNIX_EPOCH};

use prost_types::Timestamp;
use tokio::sync::{broadcast, mpsc};

pub mod api {
    tonic::include_proto!("dfcompanion.v1");
}
pub mod rpc;
pub mod web;

pub const EVENT_QUEUE_CAPACITY: usize = 50;
pub const USER_INPUT_CATEGORY: &str = "user_input";

#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    pub category: String,
    pub real_world_time_utc: Timestamp,
    pub game_world_time: Option<api::SimulationTimestamp>,
    pub text: String,
}

#[derive(Clone)]
pub struct AppState {
    pub queue_tx: mpsc::Sender<QueueItem>,
    pub notifications_tx: broadcast::Sender<String>,
}

impl AppState {
    pub fn new() -> (Self, mpsc::Receiver<QueueItem>) {
        let (queue_tx, queue_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let (notifications_tx, _) = broadcast::channel(128);

        (
            Self {
                queue_tx,
                notifications_tx,
            },
            queue_rx,
        )
    }

    pub fn notify(&self, message: impl Into<String>) {
        // No connected browser is a normal state; notifications are best-effort.
        let _ = self.notifications_tx.send(message.into());
    }
}

pub fn utc_now() -> Result<Timestamp, Box<dyn std::error::Error + Send + Sync>> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(Timestamp {
        seconds: i64::try_from(elapsed.as_secs())?,
        nanos: i32::try_from(elapsed.subsec_nanos())?,
    })
}

pub fn run_companion_worker(
    mut queue_rx: mpsc::Receiver<QueueItem>,
    notifications_tx: broadcast::Sender<String>,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    async move {
        while let Some(item) = queue_rx.recv().await {
            let response = mock_ai_response(&item);
            let _ = notifications_tx.send(response);
        }
    }
}

fn mock_ai_response(item: &QueueItem) -> String {
    if item.category == USER_INPUT_CATEGORY {
        format!("Mock AI: I received your message: {}", item.text)
    } else {
        format!(
            "Mock AI: I received a {} event: {}",
            item.category, item.text
        )
    }
}

pub fn validate_timestamp(timestamp: &Timestamp) -> bool {
    const MIN_TIMESTAMP_SECONDS: i64 = -62_135_596_800;
    const MAX_TIMESTAMP_SECONDS: i64 = 253_402_300_799;

    (MIN_TIMESTAMP_SECONDS..=MAX_TIMESTAMP_SECONDS).contains(&timestamp.seconds)
        && (0..1_000_000_000).contains(&timestamp.nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{
        companion_client::CompanionClient, companion_server::CompanionServer,
        EventSubmissionStatus, SimulationEvent, SimulationTimestamp,
    };
    use axum::{
        body::Body,
        http::{header, Request as HttpRequest, StatusCode},
    };
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;
    use tower::ServiceExt;

    #[tokio::test]
    async fn mock_plugin_event_flows_through_grpc_into_companion_queue() {
        let (state, mut queue_rx) = AppState::new();
        let (address, server_task) = start_test_grpc_server(state.clone()).await;

        let mut client = CompanionClient::connect(format!("http://{address}"))
            .await
            .unwrap();
        let response = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                real_world_time_utc: Some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 123_000_000,
                }),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_234,
                }),
                description: "A dwarf was found dead.".to_owned(),
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            EventSubmissionStatus::try_from(response.status).unwrap(),
            EventSubmissionStatus::Accepted
        );

        let queued = queue_rx.recv().await.unwrap();
        assert_eq!(queued.category, "unit_death");
        assert_eq!(queued.text, "A dwarf was found dead.");
        assert_eq!(queued.real_world_time_utc.seconds, 1_800_000_000);
        assert_eq!(queued.real_world_time_utc.nanos, 123_000_000);
        assert_eq!(queued.game_world_time.unwrap().tick, 1_234);

        server_task.abort();
    }

    #[tokio::test]
    async fn full_event_queue_responds_with_pause_request_and_notifies_ui() {
        let (state, _queue_rx) = AppState::new();
        let mut notifications = state.notifications_tx.subscribe();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            state
                .queue_tx
                .try_send(QueueItem {
                    category: "test".to_owned(),
                    real_world_time_utc: Timestamp {
                        seconds: 1_800_000_000,
                        nanos: 0,
                    },
                    game_world_time: None,
                    text: "filling queue".to_owned(),
                })
                .unwrap();
        }
        let (address, server_task) = start_test_grpc_server(state.clone()).await;
        let mut client = CompanionClient::connect(format!("http://{address}"))
            .await
            .unwrap();

        let response = client
            .submit_simulation_event(SimulationEvent {
                category: "unit_death".to_owned(),
                real_world_time_utc: Some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: 0,
                }),
                game_world_time: Some(SimulationTimestamp {
                    year: 252,
                    month: 4,
                    day: 7,
                    tick: 1_234,
                }),
                description: "queue overflow test".to_owned(),
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            EventSubmissionStatus::try_from(response.status).unwrap(),
            EventSubmissionStatus::PauseRequestedQueueFull
        );
        assert!(response.message.contains("asked to pause"));
        assert!(notifications
            .recv()
            .await
            .unwrap()
            .contains("queue is full"));

        server_task.abort();
    }

    #[tokio::test]
    async fn web_chat_input_is_enqueued_with_user_category() {
        let (state, mut queue_rx) = AppState::new();
        let response = crate::web::router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/chat")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("message=hello+Companion"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let item = queue_rx.recv().await.unwrap();
        assert_eq!(item.category, USER_INPUT_CATEGORY);
        assert_eq!(item.text, "hello Companion");
        assert!(validate_timestamp(&item.real_world_time_utc));
    }

    #[tokio::test]
    async fn web_chat_rejects_cross_origin_request() {
        let (state, mut queue_rx) = AppState::new();
        let response = crate::web::router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/chat")
                    .header(header::HOST, "127.0.0.1:3000")
                    .header(header::ORIGIN, "https://attacker.example")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("message=should+not+be+accepted"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(queue_rx.try_recv().is_err());
    }

    async fn start_test_grpc_server(
        state: AppState,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Server::builder()
            .add_service(CompanionServer::new(crate::rpc::service(state)))
            .serve_with_incoming(TcpListenerStream::new(listener));

        (address, tokio::spawn(server))
    }
}
