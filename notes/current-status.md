## Current Focus

What feature or bug we're actively working on.

- History compaction: an AI-generated rolling summary plus recent turns,
  replacing the silent 20-message truncation so long-running strategy and
  player corrections survive. Implemented across `src/ai.rs`, `src/session.rs`,
  and `src/app.rs`; 161 unit tests pass, and interactive testing past the
  compaction threshold confirms the distinction survives in play.
- A playtest caught a real bug along the way: the summary was counted in the
  token budget but never actually sent to the model, so compacted facts (e.g.
  the Therapy Squad rule) were lost. Fixed: `run_turn_with_key` now prepends
  `History::summary()` to each request as an explicit data-not-instructions
  message, covered by the `a_turns_request_includes_the_history_summary`
  regression test.
- Saving the summary with the game save remains a deliberately separate later
  task.

## Next Steps

Immediate next 1 to 3 tasks to work on.

- Address the `cargo clippy` items below and adopt clippy in the standing
  checks.

## Known Issues / Blockers

- `cargo clippy` finds numerous minor issues that should be addressed. Add this
  check to standing rules.
