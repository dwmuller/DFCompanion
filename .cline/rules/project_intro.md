# Project architecture

This is an enhancement for users playing Dwarf Fortress with the DFHack mod
framework. It provides an AI chat interface with a model that is given access to
to the state of the running game. The DFCompanion process interacts with the
user via an HTTP service, and with the game through Lua RPC and plugin scripts.

- Rust code in src/ is the main application code. It is a single binary crate.
- Lua code in lua/rpc and scripts. The Lua scripts run in an environment
  provided by DFHack and have to be copied to a specific directory in the DFHack
  installation for testing.
- The Web UI uses HTMX.

## Other resources

- The df-*.md files in the project root were written by the AI that helped
  initially set up the project. They have not been updated consistently and
  should not be considered authoritative.
- Files in notes/ are more recent and more up to date. You'll find more
  architecture discussion there.
- The project origin is a public Github project at https://github.com/dwmuller/dfcompanion.
- [Dwarf Fortress game details](https://dwarffortresswiki.org/)
- [DFHack usage and basic development information](  https://docs.dfhack.org/en/stable/)
- [DFHack source code](https://github.com/DFHack/dfhack)
    - A local workspace of a private fork can be found at ~/projects/dfhack.

## Coding standards

- Write tests for all new features.
    - Tests which require a running game are considered "live" tests and should not run by default.
- Prefer idiomatic HTMX 4 solutions over custom javascript when working on the Web UI.
- This is an unreleased application, which justifies a lack of branch
  management. All work proceeds in main unless it's unusually speculative.
- Always prefer asking for direction before making large architectural changes.
- When testing, you can request that a Dwarf Fortress game be started for you.
  You can check yourself to see if one is already running.