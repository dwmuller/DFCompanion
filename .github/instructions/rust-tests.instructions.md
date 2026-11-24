---
applyTo: "src/**/*.rs"
---

- Use descriptive test names: "should [expected behavior] when [condition]"
- Each test should verify one behavior. Multiple assertions are allowed only
  when they check different fields of the same returned value.
- Module-specific tests can reside in the same file as the module, in a 
  `#[cfg(test)] mod tests { ... }` block.
    - If the tests amount to more than 200 lines of test code, move them to a
      unit test file at `src/<module>/tests.rs`.
- Tests of interactions between modules that don't have a clearly hierarchical
  relationship are considered integration tests and should be placed under the
  top-level `tests/` directory.
- Mock dependencies external to the subject of the test, not internal modules.
  - Use hand-written mocks by default. Suggest adopting a mocking framework to
    the user if a dependency requires more than 5 mocked trait methods or needs
    call-sequence verification that a hand-written mock cannot express without
    duplicating logic.
- **Rust verification:** After editing Rust files:
  1. Run `cargo fmt`.
  2. Run `cargo check` if verifying only one or two changed files. Otherwise
     skip it.
    - Correct any issues reported by `cargo check` and re-run it. Repeat this
      at most 3 times before asking the user for help.
    - After a clean run of `cargo check`, proceed to the next step.
  3. Run `cargo clippy` to check for common mistakes and code quality issues.
    - Correct any issues reported by `cargo clippy` and re-run it. Repeat this
      at most 3 times before asking the user for help.
    - After a clean run of `cargo clippy`, proceed to the next step.
  4. Run tests using `cargo test`.
    - If a test fails, do not change the test unless the behavior being tested
      was an explicit goal of the current task. When unsure about this, ask the
      user. Otherwise, fix the code under test and rerun the tests.
    - Repeat this at most 3 times before asking the user for help.
- Live tests can be run only if a game is running and a fortress is loaded,
  using `cargo test --test live_game_tests -- --ignored`.
  - When live testing is wanted, ask the user if the game is in the required
    state first.
  - If the user confirms the game is not running or no fortress is loaded, do
    not run `cargo test --test live_game_tests -- --ignored`. Report the
    required state to the user and wait for confirmation. If the user does not
    respond, skip live tests and state that they were skipped.
