use tonic::{Request, Response, Status};

use crate::{
    api::{
        companion_server::Companion, EventSubmissionResult, EventSubmissionStatus, SimulationEvent,
    },
    AppState, GameEvent, QueueItem,
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
        let time = event
            .game_world_time
            .ok_or_else(|| Status::invalid_argument("game-world timestamp is required"))?;
        let event = GameEvent {
            id: None,
            date: format!(
                "{:03}-{:02}-{:02}:{}",
                time.year, time.month, time.day, time.tick
            ),
            report_type: None,
            category: event.category.trim().to_owned(),
            text: event.description.trim().to_owned(),
        };
        event.validate().map_err(Status::invalid_argument)?;

        match self.state.queue_tx.try_send(QueueItem::GameEvent(event)) {
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
