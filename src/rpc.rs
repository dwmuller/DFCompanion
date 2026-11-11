use tonic::{Request, Response, Status};

use crate::{
    api::{
        companion_server::Companion, EventSubmissionResult, EventSubmissionStatus, SimulationEvent,
    },
    app::CompanionApp,
    events::{GameEvent, QueueItem},
    notifications::{DisplayMessage, DisplayMessageKind},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct CompanionRpc {
    app: Arc<CompanionApp>,
}

pub fn service(state: Arc<CompanionApp>) -> CompanionRpc {
    CompanionRpc { app: state }
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

        match self.app.event_queue().try_send(QueueItem::GameEvent(event)) {
            Ok(()) => Ok(Response::new(EventSubmissionResult {
                status: EventSubmissionStatus::Accepted as i32,
                message: "event accepted".to_owned(),
            })),
            Err(message) => {
                let message = message
                    + " The plugin was asked to pause the game; this event was not accepted.";
                self.app.notifier().notify(DisplayMessage::new(
                    DisplayMessageKind::Warning,
                    message.clone(),
                ));
                Ok(Response::new(EventSubmissionResult {
                    status: EventSubmissionStatus::PauseRequestedQueueFull as i32,
                    message,
                }))
            }
        }
    }
}
