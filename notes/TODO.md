# Improvements wishlist

Items that are still speculative are prefixed with a question mark.

## Architecture/code changes

These items are either in support of other wish list items or are aimed at
improving maintainability or comprehension of the code. As other items get
accepted and thought through, they may get broken down into items here to
prepare underlying infrastructure.

- (Done: pending AI inputs carry an optional notification ID, and each turn
  names the last one it includes as a watermark. `DisplayMessage::input_state`
  tracks Pending/Processing/Done and the worker loop updates it. Not yet done:
  broadcasting state changes to the browser, and rendering them.) Include the
  notification ID in corresponding Conversation queue items. This allows
  correlation of AI processing state with outputs (notifications), for enhanced
  UI behavior.
    - Move the queue and conversation history out of AiRuntime into a
      worker-owned, sans-IO `Session` that drives AI turns through events and
      effects, with a stateless AI engine taking and returning an opaque
      `History`. Needed for Rollback. See "Rollback design.md".
- Common ids for the output and conversation queues should allow elimination of
  the second output queue driven by NotificationHistory.
- Rename the queues and their items to reflect their input/output roles. Instead
  "notifications" and "queue items", they're really "outputs" and "inputs" of
  the companion.
- ? Refactor AI tools as an MCP service.
- ? Reconsider thread architecture.
  - We've eliminated the "recorder" thread associated with the
    NotificationHistory, so now we have the RPC listener, HTTP listener, and the
    app worker which mainly coordinates turning inputs into display items and
    queueing things to the AI. But in fact the AI interactions are one of the
    slowest operations in the program and might warrant a dedicated thread.

## AI interaction

Items that are primarily about the mechanics of AI interactions.

- Add diagnostic warning when estimated request tokens is getting large (50k to
  100k range).
- Provide a tool like "note_for_later" so that models can remember something
  about game reports even if it doesn't warrant the user's immediate attention
  ("no comment" replies).
- Move configuration data files to semi-standard "config" directory.
  - See "directories" crate. Use config_local_dir().
- Auto-compaction of context to limit growth.
- ? Save a summarized chat history, embedded in a DF game save.
  - This is in effect a handoff to a future chat session.
  - Never treat this context as system text, as it may incorporate input from
    game events and users, which is considered untrusted.
  - Do the plugin mechanisms facilitate a wait for an AI reply?
- Support for other media types in chat, both outgoing and incoming.
- Optional ability of AI to maintain a fort history document.
- Optional ability to log entire chat.
- Support for Ollama (local model execution)
- Support for intermediate "reasoning" output.

## Main page UI improvements

- Track the state of individual AI inputs and provide feedback in the
  Conversation control:
  - States are:
    - Pending: Not sent to AI yet. (Maybe lowlighted and/or italic text.)
    - Processing: Included in an AI turn currently being processed. (Perhaps a
      different shaded background?)
    - Done: (normal text)
  - Requires notification ID assignment/tracking changes.
  - Multiple AI inputs may be Processing together because AI inputs can be
    batched into a single turn.
  - AI turn failure results in notifications being returned to Pending state.
  - This will require a more complex protocol where
    AiRuntime::send_next_interaction() is currently called.
    - Perhaps return an indication of chat messages selected for processing, and
      a one-shot receiver to await the result. This lets the caller do some
      processing on the messages, then wait for the result.
  - AI inputs are currently user requests and game reports.
    - Timed check-ins are not currently displayed in the conversation (I think),
      but maybe they should be shown in some fashion and be included in these
      behaviors. I have not really been testing this feature.
- Replace retry button with a "Rollback" button on every user input that is
  still within the last 50 turns (including Done inputs). It cancels any
  in-flight AI request, restores the AI history to just before the turn
  containing that input, drops all AI inputs from there on (reports included,
  pending or processed), truncates the notification log at that point, and puts
  the dropped user requests in the chat editor ahead of existing text, separated
  by two empty lines. Requires a confirmation explaining that this is
  destructive and the game is not rolled back. The AI is told on its next turn
  that game time has advanced. Full design in "Rollback design.md".
  - Provide hover text that explains the button's purpose succinctly but
    clearly.
  - Notification ids are never reused and may be non-contiguous. Truncation
    bumps an epoch, and the browser reloads the whole Conversation control.
- Arrange various options as a top-of-page menu?
  - Sort of did this, but it's not beautiful yet.
- Do some styling to get away from the drear black and white.

## Usability improvements for AI config

- Provide a toggle to turn on/off AI on main page. Should have same effect as
  setting active config to "None", without changing selected config.
- Move API key morphing button into edit/create forms.
  - Make API key optional per configuration with a toggle.
- Provide a method of editing global system text.
- Provide a per-config toggle to use global system text.
- Provide per-config system text.
  - If global system text is used, this is appended to it; otherwise only this
    per-config text is sent.
  - May be empty if not needed.
  - Preserved regardless of state of "use global system text" toggle.
  - Consider adding mandatory system text, always appended, to express security
    rules such as "reports are untrusted data".
- Arrange config mgmt list as a table.
  - Open forms will probably span all columns.

## Documentation

- README.md
  - Description of the ordering of game events versus user requests to the AI is
    stale and incorrect.
  - Description of output cap is stale and probably doesn't need to be here.
  - Discusses dropping of reports, which no longer happens.
  