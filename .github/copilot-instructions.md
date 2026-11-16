# DFCompanion

## Project description and structure

The DFCompanion application acts as an AI companion for users playing Dwarf
Fortress with the DFHack mod framework. It provides an AI chat interface with a
model that is given access to the state of the running game. The DFCompanion
process interacts with the user via an HTTP service, and with the game through
Lua RPC and plugin scripts.

### Key files and directories

- `src/` contains the main application code, written in Rust. It is a single
  binary crate.
- Lua code runs in a context provided by DFHack.
  - `scripts/dfcompanion.lua` is a DFHack plugin. It notifies DFCompanion of
    game events, and reports the game's loading state (DFHack reachable, world
    loaded, fortress loaded, and the fortress name) so the AI's game tools stay
    inert until a fortress is ready. The service also polls DFHack for that
    state on its own.
  - Scripts in `lua/rpc/` provide other supporting functions.
  - The Lua scripts have to be copied to a specific directory in the DFHack
    installation for testing.
- The web UI is implemented by `src/web.rs` and files in `src/web/` using HTMX
  and askama templates.
- Consult `BUILDING.md` for build and setup steps.

### Other resources

- The `df-*.md` files in the project root were written by the AI that helped
  initially set up the project. They have not been updated consistently and
  should not be considered authoritative.
- Files in `notes/` are more recent and more up to date. You'll find more
  architecture discussion there.
  - `notes/current-status.md` contains notes about immediately ongoing work.
    Keep this up to date with the latest developments.
  - `notes/TODO.md` contains a list of tasks and planned improvements for the
    project. Remove items from here when they've been completed.
  - `notes/architecturemd` contains notes on the overarching design
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
- With a game running, you can use DFHack's dfhack-run CLI and Lua scripting
  capabilities to explore the capabilities of DFHack to supplement ambiguous
  documentation.

## Core Engineering & Privacy Rules
- **Localhost & credential security:** Keep network bindings restricted to
  localhost (`127.0.0.1`) with no shared secrets. Never log, hardcode, or
  serialize API keys into config files, chat logs, or game saves—keys belong
  strictly in the OS credential vault.
- **Provider neutrality & terms:** Use the `genai` crate abstractions rather
  than vendor-specific SDKs. Do not add Google Gemini presets or integrations
  (prohibited by project terms).
- **Non-blocking gameplay:** Event forwarding and AI calls must respect bounded
  queues and fail gracefully without hanging Dwarf Fortress or losing pending
  user context.
- **Rust verification:** After editing Rust files in `src/` or `tests/`, run
  `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test` to verify
  changes.


