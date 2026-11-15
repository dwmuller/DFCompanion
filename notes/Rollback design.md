# Rollback design

Status: design agreed and implemented. All four staging steps are done: 1
(`History` newtype, functional `run_turn`), 2 (`Session` and the effect
interpreter), 3 (log epochs, `truncate_from`, state-change broadcast,
`rollback_floor`), and 4 (checkpoints, the `Rollback` event and route, the
button, editor prefill and confirmation). The old retry control is still
present; removing it is a follow-up. Replaces the old "Recall" items in
TODO.md.

## Goal

A "Rollback" button on every user input in the Conversation control, including
inputs that are Done. Pressing it (after a confirmation) returns the AI side of
the application to the moment just before the turn containing that input:

- The AI's conversation history is restored to what it was before that turn.
- Every AI input from that turn onward is dropped: user requests, game reports,
  processed or still pending.
- The text of the dropped *user requests* is placed in the chat editor, in
  original order, separated by two blank lines, ahead of any text already in
  the editor. Dropped game reports are not recovered and not replayed.
- The notification log is truncated at the same point, including non-AI
  messages after it. The confirmation says this is irrecoverable.
- Any in-flight AI request is cancelled.
- The game is not rolled back. The user may want to reload a save at the same
  time. The AI is told that game time has advanced.

## Decisions

- The target is a *turn*, not an input. If the selected input shares a batch
  with earlier user requests, they are rolled back too. (A hidden
  re-submission of the earlier requests is a possible later change.)
- Pending reports are dropped along with everything else after the cut.
- Reports are not replayed. The next prompt after a rollback carries a short
  app-written note: the conversation was rolled back, game time has advanced,
  reports may have been missed, and `get_recent_reports` can catch up. Sent
  once.
- Keep 50 turn checkpoints. Older inputs have no button.
- No `Revoked` input state. Revocation truncates the log.
- Rolling back while chat is Blocked clears the block.

## Architecture

### AiEngine: stateless; history is an opaque value

```rust
pub struct History(Arc<Vec<ChatMessage>>);   // internals private to the engine

async fn run_turn(&self, history: History, prompt: Prompt, cancel: CancellationToken)
    -> Result<TurnOutput, String>;

struct TurnOutput { history: History, answer: Option<String>, usage: Vec<AiTurnUsage> }
```

- The engine owns no conversation state, notifier or lock. Cancelling a turn
  cannot leave anything half-committed.
- Callers never look inside `History`. The engine may change it in ways nobody
  else sees (trimming now; later a "note for later" tool and auto-compaction).
  Whatever it returns is the new history. This is why history is not derived
  from retained inputs.
- Compaction, when it exists, is another job kind taking and returning a
  `History`, sharing the single in-flight slot.
- Cancellation races only the provider HTTP call against the token and checks
  it between tool rounds. Whether aborting a tool call mid-RPC is safe on the
  Lua side has not been verified.

### Session: owned by the worker, sans-IO

```rust
struct Session {
    queue: VecDeque<AiInput>,           // pending inputs; each has an InputId
    history: History,
    records: VecDeque<TurnRecord>,      // at most 50
    in_flight: Option<InFlight>,        // turn_id, inputs, cancel handle
    blocked: bool,
    rolled_back_note_due: bool,
    // timers: flush_at, next_check
}
struct TurnRecord {
    turn_id: u64,
    inputs: Vec<AiInput>,       // retained so user text can be returned
    history_before: History,
    first_notification: NotificationId,
}
impl Session {
    fn handle(&mut self, event: Event) -> Vec<Effect>;   // pure: no IO, no clock
}
```

Events: `UserRequest`, `Report`, `Orient` (submitted as a `UserRequest` with no
id), `FlushDue`, `CheckDue`, `TurnFinished { turn_id, result }`,
`Rollback { target, editor_text, reply }`, and the results of the spawned slow
tasks below.

Effects: `Notify(DisplayMessage)`, `TruncateLog(from)`, `TransitionInputs`,
`StartTurn { turn_id, history, prompt }`, `CancelTurn`, `ReplyEditorText`,
`PublishChatState`, `Block`, `QueryGameRunning`. Scheduling is pull-based: the
worker reads `Session::flush_deadline`/`check_deadline`, so there is no timer
effect.

The worker loop interprets effects and performs the IO. Tests drive `handle()`
directly: no tokio, timers or mock provider.

Rules:

- When idle, not blocked, the queue is eligible and a config is active: build a
  prompt synchronously from `&history` and `&mut queue`, move the batch into
  `in_flight`, emit `StartTurn`.
- `TurnFinished` for a turn that is not the current in-flight one is ignored.
- On success, in one step: commit history, push a `TurnRecord` (evict the
  oldest beyond 50), emit the reply notification, mark inputs Done. This closes
  the gap that exists today between commit, notify and state change.
- On failure: the batch returns to the front of the queue, inputs go back to
  Pending, and the session blocks.
- Chat state (Idle / Processing / Blocked) is derived from `in_flight` and
  `blocked`, and published on the existing watch channel.
- Slow work (`game_is_running`, `pause_for_ai_block`) is never awaited in the
  worker. It is an effect whose result returns as an event, so Rollback and
  cancellation are never stuck behind it.
- `TurnFinished` and `Rollback` arrive on an internal channel read with a
  biased `select!`, so they cannot wait behind a flood of game events.
- Hidden inputs (Orient) get an `InputId` from the same counter as
  notification ids so they can appear in `TurnRecord`s.

### Rollback handling

`Rollback { target }`, where `target` is the notification id of a user input:

1. Locate the turn containing it (in `records`, in flight, or still queued). If
   it is no longer eligible, reply with an error status.
2. If in flight, emit `CancelTurn`.
3. Restore `history = records[t].history_before`; truncate `records` at `t`;
   drain the queue; collect the dropped inputs. (If the target is still queued,
   history is untouched: only drain the queue from that input on.)
4. Collect the text of dropped user requests, in order, and combine it with the
   current editor text. Reply with the result.
5. Emit `TruncateLog(first_notification of the turn)` and a Status line
   describing the rollback; set `rolled_back_note_due`.
6. Clear `blocked`, then continue with normal scheduling.

### Notification log

- DONE. `NotificationHistory::truncate_from(id)` removes every entry at or
  after the cut, AI-related or not, and returns how many. Ids are never reused
  and nothing may assume contiguity (the 500-entry trim already breaks it). No
  caller yet: `Rollback` handling in step 4 is the first one.
- DONE. An `epoch` counter increments on each truncation. SSE event ids become
  `epoch:id`. A reconnecting client with a stale epoch gets a `reset`; a
  connected client gets the same `reset` when a truncation happens live. The
  `reset` listener in `app.js` clears the Conversation control, and the
  `/events` handler then replays the surviving backlog, so the page rebuilds
  from the server rather than guessing what was revoked
  (`NotificationHistory::subscribe_from`). No consumer handles partial
  revocation.
- DONE. State changes of existing messages (Pending, Processing, Done) are
  broadcast as `HistoryEvent::StateChanged` and rendered as an out-of-band swap
  of a state badge keyed by a stable DOM id (`notif-state-{id}`), closing the
  "broadcast state changes" TODO item.
- DONE. Eligibility is one watermark, `Session::rollback_floor`: the first
  notification covered by a retained checkpoint, or, before any turn has
  completed, the first still-queued input. When a checkpoint is evicted the
  floor moves, which is another publish. The worker publishes it
  (`CompanionApp::subscribe_rollback_floor`) and the page reads it over SSE
  (`/rollback-floor-events`), removing the Rollback button of any message that
  falls below it. This avoids a per-message state.

### Web layer

- DONE. `POST /rollback/{id}` takes the editor text with `hx-include` and uses
  the same cross-origin check as `/retry`. On success the reply is the textarea
  swapped out of band with the combined text; on a stale target it is plain
  text. Eligible inputs render a Rollback button with a one-sentence hover
  title and an `hx-confirm`; the confirmation says the replies and reports
  after the point are lost and cannot be recovered, and that the game is not
  rolled back. It deliberately does not show counts; a server-rendered
  `<dialog>` can add them later.
- The old `/retry` control and `QueueItem::AiRetry` are deliberately left in
  place; removing them is a follow-up once Rollback has been used.

## Staging

1. DONE. `History` newtype; functional `run_turn` with cancel token; engine no
   longer owns the conversation or notifier. Behaviour-neutral.
2. DONE. `Session` and the effect interpreter in the worker, replacing
   `flush_pending_interactions_loop`. Derived chat state. Covers the queue-move
   and shared-ids TODO items.
3. DONE. Log epochs, `truncate_from`, state-change broadcast, `rollback_floor`.
   `NotificationHistory::truncate_from` and `Session::rollback_floor` have no
   caller yet; both are built and tested as infrastructure for step 4.
4. DONE. Checkpoints (`TurnRecord`s, 50 retained, `rolled_back_note_due`),
   the `Rollback` event and its `TruncateLog`/`ReplyEditorText` effects, the
   `POST /rollback/{id}` route, the button and its confirmation, the editor
   prefill, and the rollback-floor SSE stream that removes stale buttons. The
   removed-turn text is recovered into the editor in order, ahead of whatever
   was already there. `/retry` and `QueueItem::AiRetry` were deliberately left
   in place and are removed in a follow-up.

Steps 1-3 are worthwhile even without step 4. `Session` already handles
`CancelTurn` and `Resume`, which step 4 builds on.

## Open points

- Dropped game reports are discarded. If their text should also go into the
  editor, only step 4's reply changes.
- Aborting tool calls mid-RPC needs checking against the Lua RPC layer.
- Whether periodic (`Check`) and `Orient` turns make their own checkpoints or
  fold into the preceding one. Default: their own.
