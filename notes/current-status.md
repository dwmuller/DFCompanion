# Current Status

## Current Focus

- Fortress session IDs now guard HTTP plugin interactions, chat persistence,
  and all game query and action RPCs. Revisit lifecycle polling next. A future
  foreign MCP client bypasses `rpc.dfcompanion` and will need its own session
  check.
- Player notes are persisted with chat history in version-2 Zstandard-
  compressed, base64-encoded JSON snapshots in DFHack site data. Version-1
  snapshots migrate with an empty notebook. Live testing confirmed AI note
  context, creation, reading, and editing. Verify save/restore and rollback
  with notes in a live fortress; further rollback testing is deferred. Reading
  a live legacy snapshot and migration with a mocked store are verified.
- Complete live DFHack acceptance testing for chat persistence: finish a user
  turn, save and reload, verify restored context and dates, repeat after a
  rollback, and switch between fortress saves. Review trace output for save
  overhead. Verify automatic retry after an initial restore failure during a
  fortress transition, including an unload while a restore is in flight. Verify
  fortress readiness reporting during save and return to the main menu. Two
  fortress-loaded notices were observed on one load; their cause remains
  unverified.
- Verify normal plugin event delivery after the Windows socket-timeout
  correction. The already-accepted test event was cleared from the pending
  retry slot with the player's approval.
- Verify compaction recovery with the live provider after both the original and
  no-reasoning retry exhausted the previous 2,560-token output cap.
- Exercise geometry questions with one or more AI providers. Refine the compact
  representation only if live responses show ambiguity in directions, levels,
  or place relationships.
- Verify the DFCompanion outage popup interactively with the service stopped.
  A `dfhack-run`-initiated probe exposed a popup that could not be dismissed
  and required terminating the game; confirm it can be dismissed and the game
  can be unpaused, disabled, or saved normally before treating this behavior as
  safe.