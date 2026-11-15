# Improvements wishlist

Items that are still speculative are prefixed with a question mark.

## Architecture/code changes

These items are either in support of other wish list items or are aimed at
improving maintainability or comprehension of the code. As other items get
accepted and thought through, they may get broken down into items here to
prepare underlying infrastructure.

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

- Replace retry button with a "Rollback" button on every user input that is
  still within the last 50 turns (including Done inputs). Stages 1-4 of the
  underlying work (`Session`, stateless AI engine; notification log
  epochs/truncation, input-state broadcast, rollback floor; and checkpoints,
  the `Rollback` event/route, the button and its confirmation) are done, and
  the old retry control has been removed. Full design and current status in
  "Rollback design.md".
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
  