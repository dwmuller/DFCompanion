# Current Status

## Current Focus

- Complete live DFHack acceptance testing for chat persistence: finish a user
  turn, save and reload, verify restored context and dates, repeat after a
  rollback, and switch between fortress saves. Review trace output for save
  overhead.
- Exercise geometry questions with one or more AI providers. Refine the compact
  representation only if live responses show ambiguity in directions, levels,
  or place relationships.
- Verify the DFCompanion outage popup interactively with the service stopped.
  A `dfhack-run`-initiated probe exposed a popup that could not be dismissed
  and required terminating the game; confirm it can be dismissed and the game
  can be unpaused, disabled, or saved normally before treating this behavior as
  safe.