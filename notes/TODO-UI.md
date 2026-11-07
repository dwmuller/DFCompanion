# UI improvements wish list

## Main page UI improvements

- Define and display chat state.
  - Idle, Processing, Blocked
  - Enter Blocked state when there's any failure to process AI input (not just
    user requests) and receive a response.
    - Pause the game. A prominent notification should appear in both DFC and DF
      when this happens. This is arguably an exception to the policy of not
      affecting the running game. Note that there will be no way to unpause the
      game from DFC. DFC does not need to remember whether or not it paused a
      game, though it should refrain from saying it paused it if it was already
      paused.
    - Send a System display message to indicate the reason.
    - While Blocked, do not process pending user requests, pending reports, or
      timed check-ins.
    - Entering Blocked state has no effect on the ordering of pending AI input.
    - Note that if the game is resumed, pending reports can accumulate while
      chat is blocked, and normal elision/compaction processing occurs.
  - Indicator should be hidden when Idle and there is no pending AI input.
    - Otherwise, show chat state and user pending request count, and pending
      game report count.
    - When Blocked, include a button to resume AI processing in this indicator.
      This is the only way to exit Blocked state.
- Track individual AI input states and provide feedback in the Conversation
  control:
  - Pending: Not sent to AI yet. (Maybe lowlighted and/or italic text.)
  - Processing: Included in an AI turn currently being processed. (Perhaps a
    different shaded background?)
    - Multiple AI inputs may be Processing together because AI inputs can be
      batched into a single turn. The batching policy may be refined later to
      limit turn complexity or context growth.
    - AI turn failure results in a return to Pending state, and changes the chat
      state to Blocked.
  - Done: (normal text)
  - AI inputs are currently user requests and game reports.
    - Timed check-ins are not currently displayed in the conversation (I think),
      but maybe they should be shown in some fashion and be included in these
      behaviors. I have not really been testing this feature.
    - Pending AI inputs retain their chronological ordering regardless of type.
- Replace retry button with one that pulls all pending user requests off the
  queue and places them back in the chat editor. This is independent of chat
  state.
  - These pending messages should disappear from the Conversation control
  - If the user currently has unsubmitted text in editor, the recalled messages
    precede it.
  - Each individual request (and previously unsubmitted text) is separated by
    two empty lines.
  - This has no other effect on pending AI input like game reports. If the text
    is edited and eventually resubmitted, it goes to the back of the queue as
    usual.
- Might want more categories of notifications, perhaps explicitly shown in
  conversation window.
  - Particularly want to be able to distinguish "system" msgs, of which I want
    more, from actual back-and-forth with AI.
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
- Arrange config mgmt list as a table.
  - Open forms will probably span all columns.
