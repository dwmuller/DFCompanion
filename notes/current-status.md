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
- The main page shows a discreet live DFHack/world/fortress indicator beside the
  chat state. It renders immediately and updates through an HTMX SSE stream;
  the last fortress name remains visible after unload.
- AI configurations now behave as model profiles with per-model context-window
  and output-token budgets. Older saved profiles load with 32,768 total and
  8,192 output tokens, while the model selector presents the model and route
  ahead of the profile nickname.
- Model editors can explicitly refresh suggestions through `genai` model
  discovery. Discovered IDs are sorted/deduplicated and merged with shipped
  suggestions; a saved vault key is used for authenticated profiles, while
  failures leave manual entry and existing suggestions intact. Pasted operation
  URLs ending in `/responses`, `/chat/completions`, `/messages`, or
  `/embeddings` are normalized back to the shared provider base URL.
- Model profiles include a backward-compatible reasoning mode (provider
  default, off, low, medium, high). Normal turns use the configured effort,
  while compaction falls back to low when provider default is selected.
- History compaction uses the active model's derived input budget, caps summary
  output separately, keeps four recent messages, and gives a newly compacted
  summary two growing turns before the soft threshold applies again. Hard
  context exhaustion still forces immediate compaction.
- Tool rounds are checked against the active model's input budget. Results that
  would exceed it are replaced with explicit per-tool errors, allowing the model
  to finish from partial evidence rather than blocking AI processing.
- The AI's game tools are gated on a loaded fortress: a tool call with no
  fortress loaded returns an error the model can relay, while the conversation
  itself is never paused. This fixes a crash where querying fort data during a
  fortress load could crash the game. The lifecycle flags themselves are safe to
  read at any screen, so the poller carries no such risk.
- Selected user-facing diagnostics now travel through allowlisted `user-*`
  tracing targets into the notification history. AI-usage estimate and reported
  usage messages use this path while normal console tracing remains controlled
  independently by `RUST_LOG`.
- Fort geometry is available through `get_fortress_geometry` and
  `get_floor_plan`. The overview reports per-level counts for known housing,
  zones, workshops, and stockpiles. Floor plans are bounded ASCII maps that
  replace hidden tiles with `?`, include nearby visible place footprints, and
  force complete DFHack block snapshots rather than depending on incremental
  RemoteFortressReader change tracking. The loaded-fortress end-to-end tool test
  passes.
- The DFCompanion Lua RPC implementation has been split from one 2,000-line
  file into a narrow `rpc.dfcompanion` facade and cohesive
  `dfcompanion.internal.*` modules for common helpers, lifecycle/persistence,
  creatures, work/orders, reports, items, places/geometry, squads, moods, and
  automation. Internal modules sit outside DFHack's remotely callable `rpc.*`
  namespace. The facade preserves the original exported function contract, and
  the full live-game AI tool acceptance test passes with the split modules.
- `inspect_tile(x, y, z)` is available for bounded, player-visible single-tile
  inspection. It reports terrain and sky-exposure flags, intersecting buildings
  and zones, visible creatures, non-carried items, and door/hatch/bridge state.
  Hidden tiles are redacted before any terrain or occupant data is collected.
- Resident work details now include authoritative native labor toggles,
  matching work-detail policy/membership, specialization, and active DFHack
  automation context. `list_work_details` accepts an exact normalized labor
  filter, and `get_labor_coverage` returns a paged roster of residents with a
  given labor enabled, including status, position, specialization, and
  applicable details.
- Workshop and stockpile tools now expose bidirectional routing links. Stockpile
  output additionally includes links-only mode and compact enabled/total
  sub-filter counts for accepted categories, enabling the AI to identify
  which workshops take from or give to a pile without reverse-engineering
  coordinates or job reports.
- Implemented across `src/game_state.rs`, `src/app.rs`, `src/ai_tools.rs`,
  `src/web.rs`, `src/main.rs`, `lua/plugin/dfcompanion.lua`, and
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

- Run the live DFHack acceptance sequence for chat persistence: complete a
  user turn, save and reload, verify restored context and dates, repeat after a
  rollback, and switch between two fortress saves. Then review the trace output
  for save overhead.
- Exercise geometry questions with one or more AI providers and refine the
  compact representation if the model misreads directions, levels, or place
  relationships. Revisit third-party MCP only if it provides materially better
  geometry behind the same filtering contract.
- Interactively verify the DFCompanion outage popup with the service stopped:
  after enabling the script and then triggering or submitting an event,
  confirm the popup can be dismissed and the game can be unpaused, disabled, or
  saved normally. A `dfhack-run`-initiated enable probe exposed a popup that
  could not be dismissed and required terminating the game.

## Known Issues / Blockers

- The service-outage popup needs the interactive acceptance check above before
  this behavior can be considered safe for normal use.