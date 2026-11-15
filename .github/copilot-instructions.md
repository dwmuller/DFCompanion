# DFCompanion

## Project description and structure

The DFCompanion application acts as an AI companion for users playing Dwarf
Fortress with the DFHack mod framework. It provides an AI chat interface with a
model that is given access to the state of the running game. The DFCompanion
process interacts with the user via an HTTP service, and with the game through
Lua RPC and plugin scripts.

### Key files and directories

- src/ contains the main application code, written in Rust. It is a single
  binary crate.
- Lua code runs in a context provided by DFHack.
  - scripts/dfcompanion.lua is a DFHack plugin. Its main function is to notify
    DFCompanion of game events.
  - Scripts in lua/rpc/ provide other supporting functions.
  - The Lua scripts have to be copied to a specific directory in the DFHack
    installation for testing.
- The web UI is implemented by src/web.rs and files in src/web/ using HTMX and
  askama templates.
- Consult BUILDING.md for build and setup steps.

### Other resources

- The df-*.md files in the project root were written by the AI that helped
  initially set up the project. They have not been updated consistently and
  should not be considered authoritative.
- Files in notes/ are more recent and more up to date. You'll find more
  architecture discussion there.
- The Git project's origin is a public GitHub project at
  https://github.com/dwmuller/dfcompanion.

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
  - A local workspace of a private fork can be found at ~/projects/dfhack.
