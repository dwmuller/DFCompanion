## Current Focus

What feature or bug we're actively working on.

- History compaction: replace the silent 20-message truncation with an
  AI-generated rolling summary plus recent turns, so long-running strategy and
  player corrections survive instead of being dropped.
- Goal is a session that stays strategically consistent in play; saving the
  summary with the game save is deliberately a separate later task.

## Next Steps

Immediate next 1 to 3 tasks to work on.

- Reshape `History` in `src/ai.rs` to hold a rolling summary plus recent
  messages, and stop trimming in `with_turn`.
- Add `AiRuntime::compact_history` with a dedicated summarization system prompt
  (no tools, not the configured system text), size validation, and cancellation.
- Add compaction to the `Session` state machine as a cancellable job: trigger it
  before a turn when history is large, block turns while it runs, ignore stale
  results, and cancel it on rollback; wire it through `CompanionApp::perform`.
- Reuse `History` budget accounting in `build_prompt` so the summary counts
  against `MAX_INPUT_TOKENS`, and add focused tests plus a Therapy Squad
  playtest past the old history limit.

## Known Issuess / Blockers

- TBD
