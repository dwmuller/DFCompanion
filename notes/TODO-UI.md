# UI improvements wish list

## Main page UI improvements

- Both of the next two features will require use of the id assigned by
  NotificationHistory to correlate items stored there and in the Conversation
  queue, or some other way of navigating between the two. Those identifiers
  currently reside only in NotificationHistory and in its communication with the
  HTTP service.
- Track the state of individual AI inputs and provide feedback in the
  Conversation control:
  - States are:
    - Pending: Not sent to AI yet. (Maybe lowlighted and/or italic text.)
    - Processing: Included in an AI turn currently being processed. (Perhaps a
      different shaded background?)
    - Done: (normal text) 
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
- Replace retry button with a "Recall" button that pulls all pending user
  requests out of NotificationHistory and out of the Conversation queue, and
  places them back in the chat editor. This is independent of chat state.
  - Provide hover text that explains the button's purpose succinctly but
    clearly.
  - These pending messages should disappear from the Conversation control.
  - If the user currently has unsubmitted text in editor, the recalled messages
    precede it.
  - Each individual request (and previously unsubmitted text) is separated by
    two empty lines.
  - This has no other effect on pending AI input like game reports. If the text
    is edited and eventually resubmitted, it goes to the back of the queue as
    usual.
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
