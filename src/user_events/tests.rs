use super::*;
use crate::notifications::{NotificationHistory, NotificationId};
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

fn with_layer() -> (UserEventLayer, NotificationHistory) {
    let history = NotificationHistory::new();
    let layer = UserEventLayer::new(Notifier::new(history.clone()));
    (layer, history)
}

fn attached_history() -> NotificationHistory {
    let (layer, history) = with_layer();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        emit(UserEvent::new(UserEventKind::AiUsage, "usage").with_beep(true));
    });
    history
}

#[test]
fn should_enqueue_allowlisted_user_events() {
    let history = attached_history();
    let subscription = history.subscribe_from(None);

    assert_eq!(
        subscription.backlog,
        vec![(
            1,
            DisplayMessage::new(DisplayMessageKind::AiUsage, "usage").with_beep(true)
        )]
    );
}

#[test]
fn should_ignore_non_user_targets() {
    let (layer, history) = with_layer();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: "dfcompanion", message = "internal detail");
    });

    assert!(history.subscribe_from(None).backlog.is_empty());
}

#[test]
fn should_enqueue_user_events_when_console_filter_is_none() {
    let (layer, history) = with_layer();
    let console = tracing_subscriber::fmt::layer()
        .with_writer(std::io::sink)
        .with_filter(EnvFilter::new("none"));
    let subscriber = tracing_subscriber::registry().with(console).with(layer);

    tracing::subscriber::with_default(subscriber, || {
        emit(UserEvent::new(UserEventKind::AiUsage, "visible"));
    });

    assert_eq!(
        history.subscribe_from(None).backlog[0].1.kind,
        DisplayMessageKind::AiUsage
    );
}

#[test]
fn should_ignore_allowlisted_events_without_a_message() {
    let (layer, history) = with_layer();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: "user-status", beep = false);
    });

    assert!(history.subscribe_from(None).backlog.is_empty());
}

#[test]
fn notification_ids_remain_sequential_for_user_events() {
    let (layer, history) = with_layer();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        emit(UserEvent::new(UserEventKind::Status, "one"));
        emit(UserEvent::new(UserEventKind::Warning, "two"));
    });

    let ids = history
        .subscribe_from(None)
        .backlog
        .into_iter()
        .map(|(id, _)| NotificationId(id))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![NotificationId(1), NotificationId(2)]);
}
