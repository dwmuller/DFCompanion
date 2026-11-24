# DFCompanion

## Project description and structure

The DFCompanion application acts as an AI companion for users playing Dwarf
Fortress with the DFHack mod framework. It provides an AI chat interface with a
model that is given access to the state of the running game. The DFCompanion
process interacts with the user via an HTTP service, and with the game through
Lua RPC and plugin scripts.

### Tool use during development

- Always prefer using available MCP tools over shell commands.
  - Examples:
    - Language Server Protocol (LSP) tools over string searches for
      finding usages and references in the code.
    - git tools over shell-based git commands.
    - For DFHack CLI tasks, use the DFHack MCP tool when
      available; otherwise use shell commands.
  - Known exceptions:
    - Use shell commands for cargo and CMake tasks when no MCP equivalents
      are available. 
- Rust tooling: `cargo` for building, checking, linting, and testing.
- DFHack CLI and Lua scripting for interacting with the game.
- HTTP client tools for testing the web interface.
- Git for version control and collaboration.

### Key files and directories

- `src/` contains the main application code, written in Rust. It is a single
  binary crate.
- Lua code runs in a context provided by DFHack. All lua code is under `lua/`
  - `lua/plugin/dfcompanion.lua` is a DFHack plugin. It notifies DFCompanion of
    game events, and reports the game's loading state (DFHack reachable, world
    loaded, fortress loaded, and the fortress name) so the AI's game tools stay
    inert until a fortress is ready. The service also polls DFHack for that
    state on its own.
  - Scripts in `lua/rpc/` provide other supporting functions.
  - The Lua scripts have to be copied to the DFHack installation for live
    testing. See the "Lua script installation" section under Core Engineering &
    Privacy Rules for details.
- `tests/` contains test code for the application that is not associated with a
  specific Rust module, written in Rust using the standard testing framework.
- The web UI is implemented by `src/web.rs` and files in `src/web/` using HTMX
  and askama templates.
- Consult `BUILDING.md` for build and setup steps.

### Other resources

- Files in `notes/` are more recent and more up to date. You'll find more
  architecture discussion there.
  - `notes/current-status.md` contains notes about *current* work. This includes
    tasks that are incomplete or about to be started. It should not contain
    notes about completed work -- notes of this sort should be recorded in
    architecture.md if they're significant enough to warrant explicit mention
    outside of the code. Keep this up to date.
  - `notes/TODO.md` contains a list of tasks and planned improvements for the
    project. Remove items from here when they've been completed.
  - `notes/architecture.md` contains notes on the overarching design
    decisions and principles guiding the project.
- The Git project's origin is a public GitHub project at
  `https://github.com/dwmuller/dfcompanion`.

## Coding standards

- This is an unreleased application, which justifies a lack of branch
  management. All work occurs in the main branch by default.
- Provide a brief descriptive comment on functions whose purpose is not
  immediately obvious from the name, or if the inputs, outputs, or calling
  prerequisites have noteworthy requirements.
- When uncertain about something, ask for direction.
- Do not commit changes without an explicit request to do so.

## Design resources

When strategizing about game features or the design of code that interacts with
external resources, these resources may be useful.

- [Dwarf Fortress game details](https://dwarffortresswiki.org/)
- [DFHack usage and basic development information](https://docs.dfhack.org/en/stable/)
- [DFHack source code](https://github.com/DFHack/dfhack)
  - A local workspace of a private fork can be found at `~/projects/dfhack`.
- With a game running, you may use DFHack's dfhack-run CLI and Lua scripting to
  check ambiguous documentation; if the result is still unclear, ask the user
  for direction.

## Core Engineering & Privacy Rules
- **Localhost & credential security:** Keep network bindings restricted to
  localhost (`127.0.0.1`) with no shared secrets. Never log, hardcode, or
  serialize API keys into config files, chat logs, or game saves—keys belong
  strictly in the OS credential vault.
- **Provider neutrality & terms:** Use the `genai` crate abstractions rather
  than vendor-specific SDKs. Do not add Google Gemini presets or integrations
  (prohibited by project terms).
- **Non-blocking gameplay:** Event forwarding and AI calls must respect bounded
  queues and fail without hanging Dwarf Fortress or losing pending user context.
- **Lua script installation:** After editing Lua scripts, use the PowerShell
  command `cmake --build build --config Debug --target install-dfhack-script` to
  copy them to the DFHack installation for live testing. If the command fails,
  show the full error output, state that the Lua scripts were not copied, and
  ask the user for direction before attempting any other fix or retry. Do not
  modify the DFHack installation manually.
- **Rust verification:** After editing Rust files in `src/` or `tests/`, run
  `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test` to verify
  changes. If any of these commands fails, fix the reported issues and rerun
  the failed command. Attempt at most 3 fix-and-rerun cycles per failing
  command. If the command still fails after 3 cycles, show the final error
  output, state which command failed, and ask the user for direction before
  making further changes. If a fix is unclear, ask the user for direction
  before continuing. Report the final result of each command.


