## Current Focus

What feature or bug we're actively working on.

- Game state tracking, implemented as a prelude to saving chat state with the
  game save. The companion now knows how far the game has loaded—DFHack
  reachable, no world, world loaded, or fortress loaded—plus the fortress name
  (kept after unload). State arrives two ways: `scripts/dfcompanion.lua` pushes
  it on DFHack's world/map load and unload events, and
  `CompanionApp::run_lifecycle_poller` polls DFHack about every 20 seconds as a
  fallback, so the state converges even when the service starts mid-game.
- The AI's game tools are gated on a loaded fortress: a tool call with no
  fortress loaded returns an error the model can relay, while the conversation
  itself is never paused. This fixes a crash where querying fort data during a
  fortress load could crash the game. The lifecycle flags themselves are safe to
  read at any screen, so the poller carries no such risk.
- Implemented across `src/game_state.rs`, `src/app.rs`, `src/ai_tools.rs`,
  `src/web.rs`, `src/main.rs`, `scripts/dfcompanion.lua`, and
  `lua/rpc/dfcompanion.lua`; the unit suite passes and clippy is clean.
- Removed the unused gRPC server and its build support. The `Companion`
  service in `proto/companion.proto` was never called by any client (the Lua
  script submits events over HTTP to `127.0.0.1:3000`), so the proto file,
  `src/rpc.rs`, `build.rs`, and the `tonic`/`prost`/`tonic-build`/
  `protoc-bin-vendored` dependencies are gone. The service now runs HTTP only.
  (The `prost_014` dev-dependency stays: the ignored live-game tests use it to
  walk protobuf replies by hand.)

## Next Steps

Immediate next 1 to 3 tasks to work on.

- Show the game state on the main page: an SSE stream and a small, discreet
  indicator, fed by the lifecycle watch channel (see `notes/TODO.md`).
- Save chat state with the game save. The lifecycle channel provides the
  trigger: push the compacted history snapshot to Lua when a fortress unloads
  (a save-adjacent moment) and restore it when a fortress loads. See
  `notes/foreign_mcp_planning.md` for the earlier design discussion of the
  storage mechanism (`dfhack.persistent.saveSiteDataString` via `RunLua`).

## Known Issues / Blockers

- None.