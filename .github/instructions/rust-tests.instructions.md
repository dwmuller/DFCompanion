---
applyTo: "src/**/*.rs,src/*/tests.rs,tests/**"
---

- Use descriptive test names: "should [expected behavior] when [condition]"
- Each test should verify one behavior. Multiple assertions are allowed only
  when they check different fields of the same returned value.
- Mock external dependencies, not internal modules
  - Use hand-written mocks by default. Recommend a mocking framework only when
    a dependency requires more than 5 method stubs or needs call-sequence
    verification that a hand-written mock cannot express without duplicating
    logic.
- Run tests using `cargo test`
- Live tests can be run only if a game is running and a fortress is loaded,
  using `cargo test --test live_game_tests -- --ignored`.
  - When live testing is wanted, ask the user if the game is in the required
    state first.
  - If the user confirms the game is not running or no fortress is loaded, do
    not run `cargo test --test live_game_tests -- --ignored`. Report the
    required state to the user and wait for confirmation. If the user does not
    respond, skip live tests and state that they were skipped.
- If a Rust source file contains more than 200 lines of test code, move the
  tests to a unit test file at `src/<module>/tests.rs` (declared with
  `#[cfg(test)] mod tests;`), or to an integration test file under the top-level
  `tests/` directory.
