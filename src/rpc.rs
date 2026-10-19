use tonic::{Request, Response, Status};

use crate::{
    api::{
        companion_server::Companion, EventSubmissionResult, EventSubmissionStatus, SimulationEvent,
    },
    validate_timestamp, AppState, QueueItem, AI_RETRY_CATEGORY, USER_INPUT_CATEGORY,
};

#[derive(Clone)]
pub struct CompanionRpc {
    state: AppState,
}

pub fn service(state: AppState) -> CompanionRpc {
    CompanionRpc { state }
}

#[tonic::async_trait]
impl Companion for CompanionRpc {
    async fn submit_simulation_event(
        &self,
        request: Request<SimulationEvent>,
    ) -> Result<Response<EventSubmissionResult>, Status> {
        let event = request.into_inner();
        if event.category.trim().is_empty() {
            return Err(Status::invalid_argument("event category is required"));
        }
        if matches!(
            event.category.as_str(),
            USER_INPUT_CATEGORY | AI_RETRY_CATEGORY
        ) {
            return Err(Status::invalid_argument(
                "event category is reserved for Companion chat controls",
            ));
        }
        if event.description.trim().is_empty() {
            return Err(Status::invalid_argument("event description is required"));
        }

        let real_world_time_utc = event
            .real_world_time_utc
            .ok_or_else(|| Status::invalid_argument("UTC timestamp is required"))?;
        if !validate_timestamp(&real_world_time_utc) {
            return Err(Status::invalid_argument("UTC timestamp is out of range"));
        }
        let game_world_time = event
            .game_world_time
            .ok_or_else(|| Status::invalid_argument("game-world timestamp is required"))?;

        let item = QueueItem {
            category: event.category,
            real_world_time_utc,
            game_world_time: Some(game_world_time),
            text: event.description,
        };

        match self.state.queue_tx.try_send(item) {
            Ok(()) => Ok(Response::new(EventSubmissionResult {
                status: EventSubmissionStatus::Accepted as i32,
                message: "event accepted".to_owned(),
            })),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let message = format!(
                    "The event queue is full (capacity {}). The plugin was asked to pause the game; this event was not accepted.",
                    crate::EVENT_QUEUE_CAPACITY
                );
                self.state.notify(message.clone());
                Ok(Response::new(EventSubmissionResult {
                    status: EventSubmissionStatus::PauseRequestedQueueFull as i32,
                    message,
                }))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(Status::unavailable("event queue is closed"))
            }
        }
    }
}
