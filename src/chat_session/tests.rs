use super::*;
use crate::ai::{build_prompt, history_exceeds_hard_limit, should_compact};
use crate::ai::{COMPACTION_KEEP_RECENT_MESSAGES, DEFAULT_SYSTEM_TEXT, MAX_HISTORY_MESSAGES};
use crate::notes::{Note, NoteSource, Notes};
use genai::adapter::AdapterKind;

fn configuration() -> AiConfiguration {
    AiConfiguration {
        id: "test".to_owned(),
        name: "Test".to_owned(),
        interaction_method: AdapterKind::OpenAI,
        model_id: "model".to_owned(),
        url: "http://localhost/".to_owned(),
        system_text: DEFAULT_SYSTEM_TEXT.to_owned(),
        context_window_tokens: crate::ai_config::DEFAULT_CONTEXT_WINDOW_TOKENS,
        max_output_tokens: crate::ai_config::DEFAULT_MAX_OUTPUT_TOKENS,
        reasoning_mode: crate::ai_config::ReasoningMode::Default,
    }
}

fn context(now: Instant) -> Context {
    Context {
        now,
        configuration: Ok(Some(configuration())),
        event_turns: true,
        periodic_turns: true,
        next_note_id: 1,
    }
}

fn context_with_large_input_budget(now: Instant) -> Context {
    let mut configuration = configuration();
    configuration.context_window_tokens = 2_000_000;
    Context {
        now,
        configuration: Ok(Some(configuration)),
        event_turns: true,
        periodic_turns: true,
        next_note_id: 1,
    }
}

fn report(text: &str) -> GameEvent {
    GameEvent {
        id: None,
        date: "250-01-01:0".to_owned(),
        report_type: None,
        category: "test".to_owned(),
        text: text.to_owned(),
    }
}

fn user(text: &str, id: u64) -> ChatEvent {
    ChatEvent::UserRequest {
        text: text.to_owned(),
        id: Some(NotificationId(id)),
    }
}

fn game_report(text: &str, id: u64) -> ChatEvent {
    ChatEvent::Report {
        event: report(text),
        id: Some(NotificationId(id)),
    }
}

/// The turn an effect list starts, if it starts one.
fn started_turn(effects: &[ChatEffect]) -> Option<(u64, Prompt, History, CancellationToken)> {
    effects.iter().find_map(|effect| match effect {
        ChatEffect::StartTurn {
            turn_id,
            prompt,
            history,
            cancel,
            ..
        } => Some((*turn_id, prompt.clone(), history.clone(), cancel.clone())),
        _ => None,
    })
}

fn transitions(effects: &[ChatEffect]) -> Vec<(u64, InputState, InputState)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::TransitionInputs { through, from, to } => Some((through.0, *from, *to)),
            _ => None,
        })
        .collect()
}

fn published(effects: &[ChatEffect]) -> Vec<ChatState> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::PublishChatState(state) => Some(*state),
            _ => None,
        })
        .collect()
}

fn notified(effects: &[ChatEffect], kind: DisplayMessageKind) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::Notify(message) if message.kind == kind => Some(message.text.clone()),
            _ => None,
        })
        .collect()
}

fn user_event_texts(
    effects: &[ChatEffect],
    kind: crate::user_events::UserEventKind,
) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::UserEvent(event) if event.kind == kind => Some(event.text.clone()),
            _ => None,
        })
        .collect()
}

fn persisted(effects: &[ChatEffect]) -> Vec<&History> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::PersistChatState(state) => Some(&state.history),
            _ => None,
        })
        .collect()
}

fn answer(history: &History, prompt: &Prompt, text: &str) -> Result<TurnOutput, TurnError> {
    Ok(TurnOutput {
        history: crate::ai::history_after_turn(history, prompt, text),
        answer: text.to_owned(),
        usage: Vec::new(),
        notes: None,
    })
}

fn finished(turn_id: u64, result: Result<TurnOutput, TurnError>) -> ChatEvent {
    ChatEvent::TurnFinished { turn_id, result }
}

#[test]
fn a_user_request_starts_a_turn_right_away() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));

    let (turn_id, prompt, _, _) = started_turn(&effects).expect("a turn starts");
    assert_eq!(turn_id, 0);
    assert_eq!(prompt.included_inputs, 1);
    assert_eq!(
        transitions(&effects),
        vec![(1, InputState::Pending, InputState::Processing)]
    );
    assert_eq!(published(&effects), vec![ChatState::Processing]);
    assert_eq!(session.chat_state(), ChatState::Processing);
}

#[test]
fn player_note_changes_persist_with_the_current_note_id_high_water_mark() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    context.next_note_id = 2;

    let effects = session.handle(
        &context,
        ChatEvent::RememberNote {
            id: "n1".to_owned(),
            text: "Finish the west wall.".to_owned(),
        },
    );

    assert_eq!(
        notified(&effects, DisplayMessageKind::Status),
        vec!["Saved note n1."]
    );
    let saved = effects.iter().find_map(|effect| match effect {
        ChatEffect::PersistChatState(state) => Some(state),
        _ => None,
    });
    let saved = saved.expect("the note is persisted");
    assert_eq!(saved.next_note_id, 2);
    assert_eq!(
        saved.notes.find("n1").map(Note::source),
        Some(NoteSource::Player)
    );
}

#[test]
fn rollback_restores_companion_notes_but_keeps_current_player_notes_and_id_state() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.notes = Notes::from_records(vec![
        Note::new(
            "n1".to_owned(),
            "Companion note before the turn.".to_owned(),
            NoteSource::Companion,
        )
        .unwrap(),
        Note::new(
            "n2".to_owned(),
            "Player note before the turn.".to_owned(),
            NoteSource::Player,
        )
        .unwrap(),
    ])
    .unwrap();
    let mut context = context(now);
    context.next_note_id = 5;

    let started = session.handle(&context, user("Question", 1));
    let (turn_id, prompt, history, _) = started_turn(&started).unwrap();
    session.handle(
        &context,
        finished(turn_id, answer(&history, &prompt, "Answer")),
    );
    session.notes = Notes::from_records(vec![
        Note::new(
            "n1".to_owned(),
            "Player edit to companion note.".to_owned(),
            NoteSource::Companion,
        )
        .unwrap(),
        Note::new(
            "n2".to_owned(),
            "Updated player note.".to_owned(),
            NoteSource::Player,
        )
        .unwrap(),
        Note::new(
            "n3".to_owned(),
            "New player note.".to_owned(),
            NoteSource::Player,
        )
        .unwrap(),
        Note::new(
            "n4".to_owned(),
            "New companion note.".to_owned(),
            NoteSource::Companion,
        )
        .unwrap(),
    ])
    .unwrap();

    let effects = session.handle(
        &context,
        ChatEvent::Rollback {
            target: NotificationId(1),
            editor_text: String::new(),
        },
    );

    let notes = &session.notes;
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.id(), note.text()))
            .collect::<Vec<_>>(),
        vec![
            ("n1", "Companion note before the turn."),
            ("n2", "Updated player note."),
            ("n3", "New player note.")
        ]
    );
    let saved = effects.iter().find_map(|effect| match effect {
        ChatEffect::PersistChatState(state) => Some(state),
        _ => None,
    });
    assert_eq!(saved.map(|state| state.next_note_id), Some(5));
}

#[test]
fn a_successful_turn_answers_commits_history_and_goes_idle() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "hi")),
    );

    assert_eq!(
        notified(&effects, DisplayMessageKind::Companion),
        vec!["hi"]
    );
    assert_eq!(
        transitions(&effects),
        vec![(1, InputState::Processing, InputState::Done)]
    );
    assert_eq!(published(&effects), vec![ChatState::Idle]);
    assert_eq!(session.history().len(), 2);
    assert_eq!(persisted(&effects), vec![session.history()]);
    assert_eq!(session.pending_through(), None);
    assert!(started_turn(&effects).is_none());
}

#[test]
fn input_arriving_during_a_turn_waits_and_then_gets_its_own_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("first", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let effects = session.handle(&context(now), user("second", 2));
    assert!(started_turn(&effects).is_none());
    assert!(published(&effects).is_empty());
    assert_eq!(session.pending_user_message_count(), 2);

    let effects = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "one")),
    );
    let (next_id, next_prompt, next_history, _) =
        started_turn(&effects).expect("the waiting input is sent next");
    assert_eq!(next_id, 1);
    assert_eq!(next_prompt.included_inputs, 1);
    assert_eq!(next_history.len(), 2);
    // No Idle state in between: processing continues.
    assert!(published(&effects).is_empty());
    // The finished turn's inputs are settled before the next turn takes its own.
    assert_eq!(
        transitions(&effects),
        vec![
            (1, InputState::Processing, InputState::Done),
            (2, InputState::Pending, InputState::Processing),
        ]
    );
}

#[test]
fn a_failed_turn_returns_its_inputs_to_pending_and_blocks() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, ..) = started_turn(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        finished(turn_id, Err(TurnError::Failed("boom".to_owned()))),
    );

    assert_eq!(
        transitions(&effects),
        vec![(1, InputState::Processing, InputState::Pending)]
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        ChatEffect::Block { reason } if reason.contains("AI request failed") && reason.contains("boom")
    )));
    assert_eq!(published(&effects), vec![ChatState::Blocked]);
    assert_eq!(session.pending_user_message_count(), 1);
    assert_eq!(session.pending_through(), Some(NotificationId(1)));
    assert_eq!(session.history().len(), 0);
    assert!(persisted(&effects).is_empty());
}

#[test]
fn nothing_starts_while_blocked_until_resumed() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, ..) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, Err(TurnError::Failed("boom".to_owned()))),
    );

    let effects = session.handle(&context(now), user("again", 2));
    assert!(started_turn(&effects).is_none());
    assert_eq!(session.chat_state(), ChatState::Blocked);

    let effects = session.handle(&context(now), ChatEvent::Resume);
    assert_eq!(published(&effects), vec![ChatState::Idle]);
    // Resuming alone sends nothing; the next input does, and it carries every
    // request retained during the block.
    assert!(started_turn(&effects).is_none());

    let effects = session.handle(&context(now), user("third", 3));
    let (_, prompt, ..) = started_turn(&effects).expect("the next input sends the queue");
    assert_eq!(prompt.included_inputs, 3);
}

#[test]
fn a_request_without_a_configuration_blocks_and_keeps_the_input() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    context.configuration = Ok(None);

    let effects = session.handle(&context, user("hello", 1));

    assert!(started_turn(&effects).is_none());
    assert!(effects.iter().any(|e| matches!(
        e,
        ChatEffect::Block { reason } if reason == "No active AI configuration."
    )));
    assert!(transitions(&effects).is_empty());
    assert_eq!(session.pending_through(), Some(NotificationId(1)));
    assert_eq!(session.chat_state(), ChatState::Blocked);
}

#[test]
fn a_configuration_error_blocks_with_the_error() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    context.configuration = Err("cannot read configuration".to_owned());

    let effects = session.handle(&context, user("hello", 1));

    assert!(effects.iter().any(|e| matches!(
        e,
        ChatEffect::Block { reason } if reason.contains("cannot read configuration")
    )));
}

#[test]
fn cancelling_returns_the_inputs_to_pending_without_blocking() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, _, _, cancel) = started_turn(&effects).unwrap();

    let effects = session.handle(&context(now), ChatEvent::CancelTurn);
    assert!(cancel.is_cancelled());
    // The turn is still in flight until it reports back.
    assert!(effects.is_empty());
    assert_eq!(session.chat_state(), ChatState::Processing);

    let effects = session.handle(&context(now), finished(turn_id, Err(TurnError::Cancelled)));
    assert_eq!(
        transitions(&effects),
        vec![(1, InputState::Processing, InputState::Pending)]
    );
    assert_eq!(published(&effects), vec![ChatState::Idle]);
    assert_eq!(session.pending_user_message_count(), 1);
}

#[test]
fn a_started_turn_emits_prompt_estimates_as_a_user_event() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));

    let usage = user_event_texts(&effects, crate::user_events::UserEventKind::AiUsage);
    assert_eq!(usage.len(), 1);
    assert!(usage[0].contains("Estimated prompt tokens"));
}

#[test]
fn a_completed_turn_emits_reported_usage_as_a_user_event() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let output = TurnOutput {
        history: crate::ai::history_after_turn(&history, &prompt, "done"),
        answer: "done".to_owned(),
        usage: vec![crate::ai_usage::AiTurnUsage {
            purpose: "test request".to_owned(),
            prompt_tokens: Some(12),
            completion_tokens: Some(3),
            total_reported: Some(15),
            ..Default::default()
        }],
        notes: None,
    };

    let effects = session.handle(
        &context(now),
        ChatEvent::TurnFinished {
            turn_id,
            result: Ok(output),
        },
    );

    let usage = user_event_texts(&effects, crate::user_events::UserEventKind::AiUsage);
    assert_eq!(usage.len(), 1);
    assert!(usage[0].contains("Tokens for test request"));
}

#[test]
fn cancelling_an_unnotified_turn_discards_its_hidden_input() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(
        &context(now),
        ChatEvent::UserRequest {
            text: "[Automatic session start]".to_owned(),
            id: None,
        },
    );
    let (turn_id, _, _, cancel) = started_turn(&effects).unwrap();

    session.handle(&context(now), ChatEvent::CancelUnnotifiedTurn);
    assert!(cancel.is_cancelled());
    session.handle(&context(now), finished(turn_id, Err(TurnError::Cancelled)));

    assert_eq!(session.pending_user_message_count(), 0);
    assert_eq!(session.chat_state(), ChatState::Idle);
}

#[test]
fn a_report_from_a_turn_that_is_not_in_flight_is_ignored() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let stale = session.handle(
        &context(now),
        finished(turn_id + 7, answer(&history, &prompt, "late")),
    );
    assert!(stale.is_empty());
    assert_eq!(session.chat_state(), ChatState::Processing);
    assert_eq!(session.history().len(), 0);

    // The real report still works, once; a repeat is stale.
    let done = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "hi")),
    );
    assert_eq!(notified(&done, DisplayMessageKind::Companion), vec!["hi"]);
    let repeat = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "hi again")),
    );
    assert!(repeat.is_empty());
    assert_eq!(session.history().len(), 2);
}

#[test]
fn resetting_for_a_new_fortress_cancels_old_work_and_starts_new_operation_ids() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let (old_turn_id, old_prompt, old_history, old_cancel) =
        started_turn(&session.handle(&context(now), user("old", 1))).unwrap();

    session.reset_for_fortress(now + Duration::from_secs(1));
    let stale_effects = session.handle(
        &context(now + Duration::from_secs(1)),
        finished(old_turn_id, answer(&old_history, &old_prompt, "stale")),
    );
    let (new_turn_id, _, _, _) =
        started_turn(&session.handle(&context(now + Duration::from_secs(1)), user("new", 2)))
            .unwrap();

    assert_eq!(
        (
            old_cancel.is_cancelled(),
            stale_effects.is_empty(),
            new_turn_id,
            session.pending_user_message_count(),
        ),
        (true, true, old_turn_id + 1, 1)
    );
}

#[test]
fn reports_wait_for_the_flush_timer_and_are_sent_together() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    assert_eq!(session.flush_deadline(), None);

    let effects = session.handle(&context(now), game_report("one", 1));
    // The first queued input moves the rollback floor from none to itself.
    assert_eq!(published_floors(&effects), vec![Some(NotificationId(1))]);
    assert_eq!(session.flush_deadline(), Some(now + EVENT_FLUSH_DELAY));
    // The deadline belongs to the first report; later ones do not postpone it.
    let later = now + Duration::from_secs(2);
    session.handle(&context(later), game_report("two", 2));
    assert_eq!(session.flush_deadline(), Some(now + EVENT_FLUSH_DELAY));

    let due = now + EVENT_FLUSH_DELAY;
    let effects = session.handle(&context(due), ChatEvent::FlushDue);
    let (_, prompt, ..) = started_turn(&effects).expect("the flush starts a turn");
    assert_eq!(prompt.included_inputs, 2);
    assert_eq!(session.flush_deadline(), None);
    assert_eq!(session.check_deadline(), due + PERIODIC_CHECK_INTERVAL);
}

#[test]
fn flushing_does_nothing_when_event_turns_are_off() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));

    let mut off = context(now);
    off.event_turns = false;
    let effects = session.handle(&off, ChatEvent::FlushDue);

    assert!(started_turn(&effects).is_none());
    // The report stays queued for the next turn.
    assert_eq!(session.pending_report_count(), 1);
    assert_eq!(session.flush_deadline(), None);
}

#[test]
fn flushing_does_nothing_while_blocked() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut none = context(now);
    none.configuration = Ok(None);
    session.handle(&none, user("hello", 1)); // Blocks: no configuration.
    session.handle(&context(now), game_report("one", 2));

    let effects = session.handle(&context(now), ChatEvent::FlushDue);

    assert!(started_turn(&effects).is_none());
    assert_eq!(session.pending_report_count(), 1);
}

#[test]
fn a_user_request_cancels_the_pending_flush() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));
    assert!(session.flush_deadline().is_some());

    let effects = session.handle(&context(now), user("question", 2));

    assert_eq!(session.flush_deadline(), None);
    // The queue starts with a report, so the first turn is an automated one
    // for the report alone; the request follows.
    let (_, prompt, ..) = started_turn(&effects).unwrap();
    assert_eq!(prompt.kind, crate::ai::PromptKind::Automated);
    assert_eq!(prompt.included_inputs, 1);
}

#[test]
fn a_periodic_check_asks_whether_the_game_runs_before_starting_a_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));
    let due = now + PERIODIC_CHECK_INTERVAL;

    let effects = session.handle(&context(due), ChatEvent::CheckDue);
    assert!(effects
        .iter()
        .any(|e| matches!(e, ChatEffect::QueryGameRunning)));
    assert!(started_turn(&effects).is_none());
    assert_eq!(session.check_deadline(), due + PERIODIC_CHECK_INTERVAL);

    let effects = session.handle(&context(due), ChatEvent::GameRunning(Some(false)));
    assert!(started_turn(&effects).is_none());
    let effects = session.handle(&context(due), ChatEvent::GameRunning(None));
    assert!(started_turn(&effects).is_none());

    let effects = session.handle(&context(due), ChatEvent::GameRunning(Some(true)));
    let (_, prompt, ..) = started_turn(&effects).expect("a running game gets a check");
    assert_eq!(prompt.included_inputs, 1);
    assert_eq!(session.flush_deadline(), None);
}

#[test]
fn periodic_checks_are_silent_when_off_or_without_a_configuration() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let due = now + PERIODIC_CHECK_INTERVAL;

    let mut off = context(due);
    off.periodic_turns = false;
    assert!(session.handle(&off, ChatEvent::CheckDue).is_empty());

    let mut none = context(due);
    none.configuration = Ok(None);
    assert!(session.handle(&none, ChatEvent::CheckDue).is_empty());
    assert_eq!(session.chat_state(), ChatState::Idle);
}

#[test]
fn a_no_comment_answer_to_an_automatic_turn_is_neither_shown_nor_remembered() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));
    let effects = session.handle(&context(now), ChatEvent::GameRunning(Some(true)));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "NO_COMMENT")),
    );

    assert!(notified(&effects, DisplayMessageKind::Companion).is_empty());
    assert_eq!(
        transitions(&effects),
        vec![(1, InputState::Processing, InputState::Done)]
    );
    assert_eq!(session.history().len(), 0);
}

#[test]
fn should_persist_note_changes_when_an_automatic_turn_returns_no_comment() {
    use std::sync::{atomic::AtomicU64, Arc};
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));
    let effects = session.handle(&context(now), ChatEvent::GameRunning(Some(true)));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let mut workspace =
        crate::notes::NoteWorkspace::new(Notes::default(), Arc::new(AtomicU64::new(1)));
    workspace.create("Remember this".to_owned()).unwrap();
    let mut output = answer(&history, &prompt, "NO_COMMENT").unwrap();
    output.notes = Some(workspace);
    let mut finish_context = context(now);
    finish_context.next_note_id = 2;

    let effects = session.handle(&finish_context, finished(turn_id, Ok(output)));

    assert!(effects.iter().any(|effect| matches!(effect, ChatEffect::PersistChatState(state) if state.notes.find("n1").is_some())));
}

#[test]
fn should_discard_note_changes_when_a_cancelled_turn_returns_success() {
    use std::sync::{atomic::AtomicU64, Arc};
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let mut workspace =
        crate::notes::NoteWorkspace::new(Notes::default(), Arc::new(AtomicU64::new(1)));
    workspace.create("discard".to_owned()).unwrap();
    let mut output = answer(&history, &prompt, "done").unwrap();
    output.notes = Some(workspace);
    session.handle(&context(now), ChatEvent::CancelTurn);
    let mut finish_context = context(now);
    finish_context.next_note_id = 2;

    session.handle(&finish_context, finished(turn_id, Ok(output)));

    assert!(session.notes().find("n1").is_none());
}

#[test]
fn should_preserve_pending_input_when_concurrent_notes_prevent_commit() {
    use std::sync::{atomic::AtomicU64, Arc};
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let mut workspace =
        crate::notes::NoteWorkspace::new(Notes::default(), Arc::new(AtomicU64::new(1)));
    workspace.create("staged".to_owned()).unwrap();
    session.notes = Notes::from_records(
        (2..=201)
            .map(|id| Note::new(format!("n{id}"), "player".to_owned(), NoteSource::Player).unwrap())
            .collect(),
    )
    .unwrap();
    let mut output = answer(&history, &prompt, "done").unwrap();
    output.notes = Some(workspace);
    let mut finish_context = context(now);
    finish_context.next_note_id = 202;

    session.handle(&finish_context, finished(turn_id, Ok(output)));

    assert_eq!(session.pending_user_message_count(), 1);
    assert_eq!(session.history().len(), 0);
    assert!(session.notes().find("n1").is_none());
}

#[test]
fn should_restore_companion_notes_but_keep_player_notes_when_a_turn_is_rolled_back() {
    use std::sync::{atomic::AtomicU64, Arc};
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let allocator = Arc::new(AtomicU64::new(1));
    let mut workspace = crate::notes::NoteWorkspace::new(Notes::default(), allocator);
    workspace.create("companion".to_owned()).unwrap();
    let mut player_context = context(now);
    player_context.next_note_id = 3;
    session.handle(
        &player_context,
        ChatEvent::RememberNote {
            id: "n2".to_owned(),
            text: "player".to_owned(),
        },
    );
    let mut output = answer(&history, &prompt, "done").unwrap();
    output.notes = Some(workspace);
    session.handle(&player_context, finished(turn_id, Ok(output)));

    session.handle(&player_context, rollback(1, ""));

    assert!(session.notes().find("n1").is_none());
    assert!(session.notes().find("n2").is_some());
}

#[test]
fn an_automatic_answer_is_shown_with_a_beep_and_remembered() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.handle(&context(now), game_report("one", 1));
    let effects = session.handle(&context(now), ChatEvent::GameRunning(Some(true)));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "A dwarf is in danger.")),
    );

    let beeps: Vec<bool> = effects
        .iter()
        .filter_map(|e| match e {
            ChatEffect::Notify(m) if m.kind == DisplayMessageKind::Companion => Some(m.beep),
            _ => None,
        })
        .collect();
    assert_eq!(beeps, vec![true]);
    assert_eq!(session.history().len(), 2);
}

#[test]
fn only_changes_of_chat_state_are_published() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    // Still idle: nothing to publish.
    assert!(published(&session.handle(&context(now), game_report("one", 1))).is_empty());
    assert!(published(&session.handle(&context(now), ChatEvent::Resume)).is_empty());
}

fn published_floors(effects: &[ChatEffect]) -> Vec<Option<NotificationId>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ChatEffect::PublishRollbackFloor(floor) => Some(*floor),
            _ => None,
        })
        .collect()
}

#[test]
fn the_rollback_floor_is_none_until_something_is_queued() {
    let now = Instant::now();
    let session = ChatSession::new(now);
    assert_eq!(session.rollback_floor(), None);
}

#[test]
fn the_rollback_floor_is_the_oldest_queued_input() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    // No configuration, so inputs stay queued instead of starting a turn.
    context.configuration = Ok(None);

    let effects = session.handle(&context, user("first", 1));
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    assert_eq!(published_floors(&effects), vec![Some(NotificationId(1))]);

    // A second, newer input does not move the floor.
    let effects = session.handle(&context, user("second", 2));
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    assert!(published_floors(&effects).is_empty());
}

#[test]
fn the_rollback_floor_moves_to_processing_inputs_while_a_turn_is_in_flight() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    // The input is now Processing, not Pending, but it is still queued and
    // still the floor: it has not been committed to history yet. It was
    // already published as the floor the moment it was queued, in this same
    // step, so starting the turn does not publish it again.
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    assert_eq!(published_floors(&effects), vec![Some(NotificationId(1))]);
}

#[test]
fn the_rollback_floor_stays_on_a_committed_turns_inputs() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "hi")),
    );

    // The turn is now a checkpoint, so its input is still eligible.
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    // The floor did not change, so it is not published again.
    assert!(published_floors(&effects).is_empty());
}

#[test]
fn the_rollback_floor_returns_to_a_failed_turns_inputs() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("hello", 1));
    let (turn_id, ..) = started_turn(&effects).unwrap();
    // Already the floor while in flight: failure does not move it.
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));

    let effects = session.handle(
        &context(now),
        finished(turn_id, Err(TurnError::Failed("boom".to_owned()))),
    );

    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    assert!(published_floors(&effects).is_empty());
}

fn editor_reply(effects: &[ChatEffect]) -> Option<Result<String, String>> {
    effects.iter().find_map(|effect| match effect {
        ChatEffect::ReplyEditorText(result) => Some(result.clone()),
        _ => None,
    })
}

fn truncated_from(effects: &[ChatEffect]) -> Option<NotificationId> {
    effects.iter().find_map(|effect| match effect {
        ChatEffect::TruncateLog(id) => Some(*id),
        _ => None,
    })
}

fn rollback(target: u64, editor_text: &str) -> ChatEvent {
    ChatEvent::Rollback {
        target: NotificationId(target),
        editor_text: editor_text.to_owned(),
    }
}

#[test]
fn rolling_back_to_a_committed_turn_restores_its_history_and_drops_later_work() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("one", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "a")),
    );

    let effects = session.handle(&context(now), user("two", 2));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "b")),
    );
    assert_eq!(session.history().len(), 4);

    let effects = session.handle(&context(now), rollback(1, "typed"));

    // History is back to before the first turn; both dropped requests are
    // recovered in order, ahead of what was already typed.
    assert_eq!(session.history().len(), 0);
    assert_eq!(truncated_from(&effects), Some(NotificationId(1)));
    assert_eq!(
        editor_reply(&effects),
        Some(Ok("one\n\n\ntwo\n\n\ntyped".to_owned()))
    );
    assert_eq!(notified(&effects, DisplayMessageKind::Status).len(), 1);
    assert_eq!(session.rollback_floor(), None);
    assert_eq!(persisted(&effects), vec![session.history()]);
}

#[test]
fn rolling_back_into_the_in_flight_batch_cancels_the_turn_and_drops_the_queue() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("one", 1));
    let (_, _, _, cancel) = started_turn(&effects).unwrap();
    session.handle(&context(now), game_report("report", 2));
    session.handle(&context(now), user("three", 3));

    let effects = session.handle(&context(now), rollback(1, ""));

    assert!(cancel.is_cancelled());
    assert_eq!(truncated_from(&effects), Some(NotificationId(1)));
    // Reports are not recovered; only the dropped requests appear.
    assert_eq!(
        editor_reply(&effects),
        Some(Ok("one\n\n\nthree".to_owned()))
    );
    assert_eq!(session.chat_state(), ChatState::Idle);
    assert_eq!(session.pending_user_message_count(), 0);
    assert_eq!(session.pending_report_count(), 0);
}

#[test]
fn rolling_back_to_a_queued_input_leaves_the_in_flight_turn_alone() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("one", 1));
    let (_, _, _, cancel) = started_turn(&effects).unwrap();
    session.handle(&context(now), user("two", 2));

    let effects = session.handle(&context(now), rollback(2, "typed"));

    assert!(!cancel.is_cancelled());
    assert_eq!(session.chat_state(), ChatState::Processing);
    assert_eq!(truncated_from(&effects), Some(NotificationId(2)));
    assert_eq!(
        editor_reply(&effects),
        Some(Ok("two\n\n\ntyped".to_owned()))
    );
    // The in-flight input is untouched and is still the floor.
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
    assert_eq!(session.pending_user_message_count(), 1);
}

#[test]
fn rolling_back_to_an_ineligible_target_errors_without_changing_anything() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("one", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "a")),
    );

    let effects = session.handle(&context(now), rollback(999, "typed"));

    assert!(matches!(editor_reply(&effects), Some(Err(_))));
    assert!(truncated_from(&effects).is_none());
    assert_eq!(session.history().len(), 2);
    assert_eq!(session.rollback_floor(), Some(NotificationId(1)));
}

#[test]
fn rolling_back_clears_a_block() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    context.configuration = Ok(None);
    session.handle(&context, user("one", 1));
    assert_eq!(session.chat_state(), ChatState::Blocked);

    let effects = session.handle(&context, rollback(1, ""));

    assert_eq!(session.chat_state(), ChatState::Idle);
    assert_eq!(truncated_from(&effects), Some(NotificationId(1)));
    assert_eq!(editor_reply(&effects), Some(Ok("one".to_owned())));
}

#[test]
fn a_rollback_note_is_sent_once_on_the_next_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let effects = session.handle(&context(now), user("one", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "a")),
    );
    session.handle(&context(now), rollback(1, ""));

    // The first turn after the rollback carries the note.
    let effects = session.handle(&context(now), user("two", 2));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    assert!(prompt.included_rollback_note);
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "b")),
    );

    // The next one does not.
    let effects = session.handle(&context(now), user("three", 3));
    let (_, prompt, _, _) = started_turn(&effects).unwrap();
    assert!(!prompt.included_rollback_note);
}

#[test]
fn a_restoration_note_is_sent_once_on_the_next_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    session.restore_chat_state(
        History::with_test_summary("Durable strategy.", &History::default()),
        Notes::default(),
        "250-01-01:0".to_owned(),
        "250-02-03:400".to_owned(),
    );

    let effects = session.handle(&context(now), user("one", 1));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    assert!(prompt.included_restoration_note);
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "a")),
    );

    let effects = session.handle(&context(now), user("two", 2));
    let (_, prompt, _, _) = started_turn(&effects).unwrap();
    assert!(!prompt.included_restoration_note);
}

#[test]
fn only_the_newest_fifty_turns_can_be_rolled_back() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    for id in 1..=MAX_TURN_RECORDS as u64 + 1 {
        let effects = session.handle(&context(now), user(&format!("m{id}"), id));
        complete_pending_work(&mut session, now, effects);
    }

    // The oldest turn was evicted, so the floor moved to the second oldest and
    // the first input is no longer eligible.
    assert_eq!(session.rollback_floor(), Some(NotificationId(2)));
    let effects = session.handle(&context(now), rollback(1, ""));
    assert!(matches!(editor_reply(&effects), Some(Err(_))));
}

/// The compaction an effect list starts, if it starts one.
fn started_compaction(effects: &[ChatEffect]) -> Option<(u64, History, CancellationToken)> {
    effects.iter().find_map(|effect| match effect {
        ChatEffect::StartCompaction {
            compaction_id,
            history,
            cancel,
            ..
        } => Some((*compaction_id, history.clone(), cancel.clone())),
        _ => None,
    })
}

/// A compaction result that summarizes `recent` under `summary`.
fn compacted(summary: &str, recent: &History) -> Result<CompactionOutput, TurnError> {
    Ok(CompactionOutput {
        history: History::with_test_summary(summary, recent),
        usage: Vec::new(),
    })
}

fn compaction_finished(
    compaction_id: u64,
    result: Result<CompactionOutput, TurnError>,
) -> ChatEvent {
    ChatEvent::CompactionFinished {
        compaction_id,
        result,
    }
}

/// Completes turns until `session` needs to compact before the next turn.
fn drive_past_compaction_threshold(session: &mut ChatSession, now: Instant) {
    for id in 1..=((MAX_HISTORY_MESSAGES as u64 / 2) + 1) {
        let effects = session.handle(&context(now), user(&format!("m{id}"), id));
        let (turn_id, prompt, history, _) =
            started_turn(&effects).expect("a turn starts before the threshold");
        session.handle(
            &context(now),
            finished(turn_id, answer(&history, &prompt, "ok")),
        );
    }
    assert!(should_compact(session.history(), &configuration()));
}

/// Completes whatever work the effects start — a turn or a compaction — until
/// nothing more is pending, as the real worker would.
fn complete_pending_work(session: &mut ChatSession, now: Instant, mut effects: Vec<ChatEffect>) {
    loop {
        if let Some((turn_id, prompt, history, _)) = started_turn(&effects) {
            effects = session.handle(
                &context(now),
                finished(turn_id, answer(&history, &prompt, "ok")),
            );
            continue;
        }
        if let Some((compaction_id, history, _)) = started_compaction(&effects) {
            effects = session.handle(
                &context(now),
                compaction_finished(compaction_id, compacted("running summary", &history)),
            );
            continue;
        }
        break;
    }
}

#[test]
fn a_large_history_triggers_compaction_before_the_next_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    // Complete turns up to the message limit: exactly 20 messages, so no
    // compaction is needed yet even though the verbatim window is smaller.
    for id in 1..=(MAX_HISTORY_MESSAGES as u64 / 2) {
        let effects = session.handle(&context(now), user(&format!("m{id}"), id));
        let (turn_id, prompt, history, _) = started_turn(&effects).expect("a turn starts");
        session.handle(
            &context(now),
            finished(turn_id, answer(&history, &prompt, "ok")),
        );
    }
    assert_eq!(session.history().len(), MAX_HISTORY_MESSAGES);
    assert!(!should_compact(session.history(), &configuration()));

    // One more turn pushes the history past the limit.
    let effects = session.handle(&context(now), user("extra", 900));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    session.handle(
        &context(now),
        finished(turn_id, answer(&history, &prompt, "ok")),
    );
    assert!(should_compact(session.history(), &configuration()));

    // The next input compacts first; its turn waits for the result.
    let effects = session.handle(&context(now), user("next", 901));
    let (compaction_id, _, _) = started_compaction(&effects).expect("compaction starts");
    assert!(started_turn(&effects).is_none());
    assert_eq!(session.pending_user_message_count(), 1);

    let current = session.history().clone();
    let effects = session.handle(
        &context(now),
        compaction_finished(compaction_id, compacted("running summary", &current)),
    );
    assert!(started_turn(&effects).is_some());
    assert_eq!(session.history().summary(), "running summary");
    assert_eq!(session.history().len(), COMPACTION_KEEP_RECENT_MESSAGES);
    assert_eq!(notified(&effects, DisplayMessageKind::Status).len(), 1);
}

#[test]
fn a_turn_compacts_when_its_snapshot_exceeds_the_byte_limit() {
    let now = Instant::now();
    let context = context_with_large_input_budget(now);
    let large_text = "\0".repeat(100_000);
    let mut seed_session = ChatSession::new(now);
    let effects = seed_session.handle(&context, user(&large_text, 1));
    let (_, prompt, history, _) = started_turn(&effects).unwrap();
    let history = crate::ai::history_after_turn(&history, &prompt, "seed");
    assert!(!history_exceeds_snapshot_budget(&history));

    let mut session = ChatSession::new(now);
    session.restore_chat_state(
        history,
        Notes::default(),
        "250-01-01:0".to_owned(),
        "250-01-01:1".to_owned(),
    );
    let effects = session.handle(&context, user(&large_text, 2));
    let (turn_id, prompt, history, _) = started_turn(&effects).unwrap();
    let effects = session.handle(
        &context,
        finished(turn_id, answer(&history, &prompt, "done")),
    );

    assert!(crate::ai::history_exceeds_snapshot_limit(session.history()));
    assert!(started_compaction(&effects).is_some());
    assert!(persisted(&effects).is_empty());
}

#[test]
fn a_successful_compaction_replaces_history_and_resumes_the_turn() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    drive_past_compaction_threshold(&mut session, now);
    let before = session.history().clone();

    let effects = session.handle(&context(now), user("next", 500));
    let (compaction_id, _, _) = started_compaction(&effects).expect("compaction starts");

    let effects = session.handle(
        &context(now),
        compaction_finished(compaction_id, compacted("SUMMARY", &before)),
    );

    assert_eq!(session.history().summary(), "SUMMARY");
    assert_eq!(session.history().len(), COMPACTION_KEEP_RECENT_MESSAGES);
    assert!(started_turn(&effects).is_some());
    assert_eq!(notified(&effects, DisplayMessageKind::Status).len(), 1);
    assert_eq!(persisted(&effects), vec![session.history()]);
}

#[test]
fn a_successful_compaction_should_allow_two_growing_turns_before_soft_compaction_again() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    let mut configuration = configuration();
    configuration.context_window_tokens = 32_768;
    configuration.max_output_tokens = 4_096;
    context.configuration = Ok(Some(configuration.clone()));
    drive_past_compaction_threshold(&mut session, now);
    let before = session.history().clone();
    let first_queue = VecDeque::from([AiInput::UserRequest {
        text: "next".to_owned(),
        id: None,
    }]);
    let mut item_count: usize = 2_300;
    let (summary, after_first) = loop {
        let summary = (0..item_count)
            .map(|i| format!("item{i} "))
            .collect::<String>();
        let candidate = History::with_test_summary(&summary, &before);
        let first_prompt =
            build_prompt(&candidate, &first_queue, &configuration, false, None).unwrap();
        let after_first = crate::ai::history_after_turn(&candidate, &first_prompt, "answer");
        let hard_limit_reached = history_exceeds_hard_limit(&after_first, &configuration);
        if should_compact(&after_first, &configuration) && !hard_limit_reached {
            break (summary, after_first);
        }
        item_count = if hard_limit_reached {
            item_count.saturating_sub(100)
        } else {
            item_count + 100
        };
    };
    assert!(should_compact(&after_first, &configuration));
    assert!(!history_exceeds_hard_limit(&after_first, &configuration));

    let effects = session.handle(&context, user("next", 500));
    let (compaction_id, _, _) = started_compaction(&effects).expect("compaction starts");
    let effects = session.handle(
        &context,
        compaction_finished(compaction_id, compacted(&summary, &before)),
    );
    let (turn_id, prompt, history, _) =
        started_turn(&effects).expect("the first normal turn is allowed after compaction");
    session.handle(
        &context,
        finished(turn_id, answer(&history, &prompt, "answer")),
    );
    assert!(should_compact(session.history(), &configuration));

    let effects = session.handle(&context, user("after cooldown", 501));
    let (turn_id, prompt, history, _) =
        started_turn(&effects).expect("the second growing turn is still allowed");
    session.handle(
        &context,
        finished(turn_id, answer(&history, &prompt, "second answer")),
    );

    let effects = session.handle(&context, user("compaction due", 502));
    assert!(started_compaction(&effects).is_some());
    assert!(started_turn(&effects).is_none());
}

#[test]
fn hard_limit_should_override_cooldown_even_without_older_messages() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    let mut context = context(now);
    let mut configuration = configuration();
    configuration.context_window_tokens = 16_384;
    configuration.max_output_tokens = 8_192;
    context.configuration = Ok(Some(configuration));
    session.restore_chat_state(
        History::with_test_summary(
            &(0..4_000).map(|i| format!("item{i} ")).collect::<String>(),
            &History::default(),
        ),
        Notes::default(),
        "250-01-01:0".to_owned(),
        "250-01-02:0".to_owned(),
    );

    let effects = session.handle(&context, user("summarize again", 500));

    assert!(started_compaction(&effects).is_some());
    assert!(started_turn(&effects).is_none());
}

#[test]
fn a_failed_compaction_blocks_without_changing_history() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    drive_past_compaction_threshold(&mut session, now);
    let before = session.history().clone();

    let effects = session.handle(&context(now), user("next", 500));
    let (compaction_id, _, _) = started_compaction(&effects).unwrap();

    let effects = session.handle(
        &context(now),
        compaction_finished(compaction_id, Err(TurnError::Failed("boom".to_owned()))),
    );

    assert_eq!(session.chat_state(), ChatState::Blocked);
    assert_eq!(session.history().summary(), before.summary());
    assert_eq!(session.history().len(), before.len());
    assert!(effects.iter().any(|e| matches!(
        e,
        ChatEffect::Block { reason } if reason.contains("compaction failed") && reason.contains("boom")
    )));
}

#[test]
fn a_cancelled_compaction_does_not_block_or_change_history() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    drive_past_compaction_threshold(&mut session, now);
    let before = session.history().clone();

    let effects = session.handle(&context(now), user("next", 500));
    let (compaction_id, _, _) = started_compaction(&effects).unwrap();

    session.handle(
        &context(now),
        compaction_finished(compaction_id, Err(TurnError::Cancelled)),
    );

    assert_eq!(session.chat_state(), ChatState::Idle);
    assert_eq!(session.history().summary(), before.summary());
    assert_eq!(session.history().len(), before.len());
}

#[test]
fn a_stale_compaction_result_is_ignored() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    drive_past_compaction_threshold(&mut session, now);
    let before = session.history().clone();

    let effects = session.handle(&context(now), user("next", 500));
    let (compaction_id, _, _) = started_compaction(&effects).unwrap();

    // A rollback cancels the compaction and restores older history.
    session.handle(&context(now), rollback(1, ""));
    let restored = session.history().clone();
    assert_eq!(restored.len(), 0);

    // The late result must not be applied.
    session.handle(
        &context(now),
        compaction_finished(compaction_id, compacted("STALE", &before)),
    );

    assert_eq!(session.history().summary(), restored.summary());
    assert_ne!(session.history().summary(), "STALE");
    assert_eq!(session.history().len(), restored.len());
}

#[test]
fn rollback_cancels_in_flight_compaction() {
    let now = Instant::now();
    let mut session = ChatSession::new(now);
    drive_past_compaction_threshold(&mut session, now);

    let effects = session.handle(&context(now), user("next", 500));
    let (_, _, cancel) = started_compaction(&effects).unwrap();
    assert!(!cancel.is_cancelled());

    session.handle(&context(now), rollback(1, ""));

    assert!(cancel.is_cancelled());
}
