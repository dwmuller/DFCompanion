# UI improvements wish list

## Main page UI improvements

- Display chat state above the Chat control, next to its title.
  - Idle, Processing, Blocked
  - When Idle, hide this element.
  - When Processing, show "Waiting for AI response".
  - When Blocked, "AI processing is blocked", and a button that effects a call
    to CompanionApp::resume_chat_if_blocked().
- Enhance chat state change behavior:
  - When entering Blocked state, pause DF.
    - Add a function in GameState to interrogate game running state and to pause
      it. Do not provide a method of unpausing. DFC does not need to remember
      whether or not it paused a game, though it should refrain from saying it
      paused it if it was already paused.
    - Ensure that when DFCompanion pauses the game, the plugin displays a popup
      to the user notifying them of this.
    - Send a similar message to DFC's notification queue as a warning. If available, include the reason for the block.
      - Consider changing GameState to a struct so the Blocked state can carry
        reason text.
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
- Replace retry button with a "Recall" button  that pulls all pending user
  requests off the queue and places them back in the chat editor. This is
  independent of chat state.
  - Provide hover text that explains the button's purpose succinctly but
    clearly.
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
