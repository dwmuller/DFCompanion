use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

use crate::notifications::{DisplayMessage, DisplayMessageKind, Notifier};

/// A selected tracing event intended for the player's notification history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserEvent {
    pub kind: UserEventKind,
    pub text: String,
    pub beep: bool,
}

impl UserEvent {
    pub fn new(kind: UserEventKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            beep: false,
        }
    }

    pub fn with_beep(mut self, beep: bool) -> Self {
        self.beep = beep;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserEventKind {
    AiUsage,
    Status,
    Warning,
    Diagnostic,
}

impl UserEventKind {
    pub(crate) fn display_kind(self) -> DisplayMessageKind {
        match self {
            Self::AiUsage => DisplayMessageKind::AiUsage,
            Self::Status => DisplayMessageKind::Status,
            Self::Warning => DisplayMessageKind::Warning,
            Self::Diagnostic => DisplayMessageKind::Diagnostic,
        }
    }
}

/// Emits a user-visible event. The target is deliberately static so the
/// subscriber can allowlist categories without parsing application messages.
pub fn emit(event: UserEvent) {
    match event.kind {
        UserEventKind::AiUsage => {
            tracing::debug!(
                target: "user-ai-usage",
                message = %event.text,
                beep = event.beep
            );
        }
        UserEventKind::Status => {
            tracing::debug!(
                target: "user-status",
                message = %event.text,
                beep = event.beep
            );
        }
        UserEventKind::Warning => {
            tracing::debug!(
                target: "user-warning",
                message = %event.text,
                beep = event.beep
            );
        }
        UserEventKind::Diagnostic => {
            tracing::debug!(
                target: "user-diagnostic",
                message = %event.text,
                beep = event.beep
            );
        }
    }
}

/// Converts allowlisted `user-*` tracing targets into UI notifications.
pub struct UserEventLayer {
    notifier: Notifier,
}

impl UserEventLayer {
    pub fn new(notifier: Notifier) -> Self {
        Self { notifier }
    }
}

impl<S> Layer<S> for UserEventLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let kind = match event.metadata().target() {
            "user-ai-usage" => UserEventKind::AiUsage,
            "user-status" => UserEventKind::Status,
            "user-warning" => UserEventKind::Warning,
            "user-diagnostic" => UserEventKind::Diagnostic,
            _ => return,
        };
        let Some(message) = event_message(event) else {
            return;
        };
        let Some(text) = message.text else {
            return;
        };
        self.notifier
            .notify(DisplayMessage::new(kind.display_kind(), text).with_beep(message.beep));
    }
}

#[derive(Default)]
struct EventMessage {
    text: Option<String>,
    beep: bool,
}

impl Visit for EventMessage {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.text = Some(value.to_owned());
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "beep" {
            self.beep = value;
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.text = Some(format!("{value:?}"));
        }
    }
}

fn event_message(event: &Event<'_>) -> Option<EventMessage> {
    let mut message = EventMessage::default();
    event.record(&mut message);
    message.text.as_ref()?;
    Some(message)
}

#[cfg(test)]
mod tests;
