## Current Focus

What feature or bug we're actively working on.

- Chat history persistence is implemented. `rpc.dfcompanion` stores a versioned
  snapshot in DFHack persistent site data after successful user turns,
  compaction, and rollback. It includes the site ID, save-time game date,
  rolling summary, and recent user/assistant messages, but no configuration,
  credentials, pending work, tool data, or visible notification transcript.
- Fortress lifecycle events now use the main worker's priority ingress. Loading
  a fortress clears older normal input, resets fortress-scoped session/UI
  state, restores that site's history asynchronously, and resumes normal input
  only after restoration. Startup uses this same path; there is no separate
  `check_game_at_startup` flow.
- The companion tracks the stable site ID as well as load state and fortress
  name. DFHack lifecycle pushes and the 20-second polling fallback both enter
  through the worker, so initial loads, saves, reloads, and site switches share
  the same ordering rules.
- Saves are serialized by the worker with one in-flight request and one
  replaceable pending snapshot. Generation/site checks and a Lua-side site
  recheck reject stale writes. Trace-level spans cover state load/save duration;
  snapshot contents are not logged.
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
- Run the live DFHack acceptance sequence for chat persistence: complete a
  user turn, save and reload, verify restored context and dates, repeat after a
  rollback, and switch between two fortress saves. Then review the trace output
  for save overhead.

## Known Issues / Blockers

- None.