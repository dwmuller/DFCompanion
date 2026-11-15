---
applyTo: "src/**/*.rs,src/*/tests.rs,tests/**"
---

- Use descriptive test names: "should [expected behavior] when [condition]"
- One assertion per test when possible
- Mock external dependencies, not internal modules
  - Use hand-written mocking tools unless you encounter a particular complex
    need, in which case recommend the addition of a mocking framework.
- Run tests using `cargo test`
- Live tests can be run only if a game is running and a fortress is loaded,
  using `cargo test --test live_game_tests -- --ignored`.
  - When live testing is wanted, ask the user if the game is in the required
    state first.
- If a Rust source file is becoming bloated with tests
  consider moving them to a dedicated `tests/` module or file.
