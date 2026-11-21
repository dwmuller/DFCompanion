# Improvements wishlist

Items that are still speculative are prefixed with a question mark.

## Architecture/code changes

These items are either in support of other wish list items or are aimed at
improving maintainability or comprehension of the code. As other items get
accepted and thought through, they may get broken down into items here to
prepare underlying infrastructure.

- Move AiUsage output items to debug! events.
- Add info, trace, and debug events to instrument critical program activity.
- ? Implement an optional tracing subscriber that enqueues OutputItems.
- Revisit the game time representation. Consider a machine-readable form of year
  plus ticks since the start of that year, and/or a fixed-width, zero-padded
  string format (for example `YYYYY-MM-DD:TTTT`) so that dates sort correctly as
  plain strings. The current `YYYY-MM-DD[:T]` string has variable-width parts.
- ? Refactor AI tools as an MCP service.

## AI interaction

Items that are primarily about the mechanics of AI interactions.

- The optional periodic check-in with the AI is currently probably not effective
  -- it seems to only check for already-enqueued activity without generating a
  prompt to the AI.
  - Replace it with a check-in based on elapsed *game time*, which sends the
    current time to the AI and instruct it to take the opportunity to check for
    significant for changes.
  - Run this once per game day.
- Provide a tool like "note_for_later" so that models can remember something
  about game reports even if it doesn't warrant the user's immediate attention
  ("no comment" replies).
- Support for other media types in chat, both outgoing and incoming.
- Optional ability of AI to maintain a fort history document.
- Optional ability to log entire chat.
- Support for Ollama (local model execution)
- Support for intermediate "reasoning" output.
- Investigate the use of foreign Dwarf Fortress MCP services.
  - See `notes/foreign_mcp_planning.md`

## Main page UI improvements

- Show the current game state discreetly: DFHack reachable or not, whether a
  world and a fortress are loaded, and the fortress name (kept after unload).
  The companion already tracks this in a watch channel; it needs an SSE stream
  (like `/chat-state`) and a small indicator.d
  - Try right-justified beside the chat state indicator.
- Arrange various options as a top-of-page menu?
  - Sort of did this, but it's not beautiful yet.
- Do some styling to get away from the drear black and white.
- Elide repetitive headers in Conversation transcript.
  - Especially for game events, but possibly others as well.

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
- Get a start on user-oriented documentation.

## Deployment

- Need a LICENSE file.
- ? Automated builds on GitHub.
- Build a method of release and distribution.
  - ? Via winget -- easy to explain, should work on all current Windows systems.
  - Figure out an affordable method of providing a trusted image/installation.
    - Look up discussion of this topic by Steve Gibson on Security Now!.

## AI configuration for development tasks

- ? Provide the official Git MCP server.
  - Might reduce the use of custom Git commands via shell.